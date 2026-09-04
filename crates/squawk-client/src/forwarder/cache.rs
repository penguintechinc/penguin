//! A real, TTL-respecting, bounded answer cache for [`crate::forwarder`].
//!
//! Go's forwarder has **no cache at all** — every query round-trips
//! upstream via DoH — despite the module advertising a `cache.enabled`
//! toggle and shipping `cache stats`/`cache flush` commands that return
//! canned text (see `docs/PARITY.md`). This module makes those commands
//! real: [`Cache::stats`] reports live counters and [`Cache::flush`]
//! actually empties the map.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::doh::DnsRecord;

/// Cache tuning, exposed for [`crate::forwarder::Forwarder`] construction.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Whether the cache is consulted/populated at all. `false` makes
    /// [`Cache::get`] always miss and [`Cache::insert`] a no-op, without
    /// changing any other forwarder behavior.
    pub enabled: bool,
    /// The maximum number of distinct `(name, type)` entries retained.
    /// Insertion beyond this bound evicts the oldest surviving entry
    /// first (FIFO) — see [`Cache::insert`].
    pub max_entries: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        CacheConfig {
            enabled: true,
            max_entries: 10_000,
        }
    }
}

/// A cached, positive DoH answer set for one `(name, type)` query.
#[derive(Debug, Clone)]
pub struct CachedAnswer {
    /// Always `0` (`NOERROR`) — only successful lookups with at least one
    /// answer are ever cached, see [`Cache::insert`].
    pub status: i32,
    pub answers: Vec<DnsRecord>,
}

/// A point-in-time snapshot of cache activity, for the module's `cache
/// stats` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    name: String,
    record_type: String,
}

impl CacheKey {
    fn new(name: &str, record_type: &str) -> CacheKey {
        CacheKey {
            name: name.to_ascii_lowercase(),
            record_type: record_type.to_ascii_uppercase(),
        }
    }
}

struct CacheEntry {
    answer: CachedAnswer,
    expires_at: Instant,
}

struct CacheState {
    entries: HashMap<CacheKey, CacheEntry>,
    /// Insertion order, for FIFO eviction once `max_entries` is reached —
    /// simpler than a full LRU and sufficient for a forwarder cache, whose
    /// hot set is dominated by TTL expiry rather than capacity pressure in
    /// normal operation.
    order: VecDeque<CacheKey>,
}

/// A thread-safe, TTL-respecting answer cache keyed on `(name, record
/// type)`. Cheap to call from multiple concurrent forwarder tasks: every
/// operation takes the lock for O(1) map work only, never across an `.await`.
pub struct Cache {
    enabled: bool,
    max_entries: usize,
    state: Mutex<CacheState>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl Cache {
    pub fn new(config: CacheConfig) -> Cache {
        Cache {
            enabled: config.enabled,
            max_entries: config.max_entries.max(1),
            state: Mutex::new(CacheState {
                entries: HashMap::new(),
                order: VecDeque::new(),
            }),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Looks up `(name, record_type)`. A disabled cache always misses. An
    /// expired entry is evicted on this read and also counts as a miss.
    pub fn get(&self, name: &str, record_type: &str) -> Option<CachedAnswer> {
        if !self.enabled {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        let key = CacheKey::new(name, record_type);
        let mut state = self.lock_state();

        let Some(entry) = state.entries.get(&key) else {
            drop(state);
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };

        if Instant::now() >= entry.expires_at {
            state.entries.remove(&key);
            drop(state);
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        let answer = entry.answer.clone();
        drop(state);
        self.hits.fetch_add(1, Ordering::Relaxed);
        Some(answer)
    }

    /// Stores a positive answer, expiring after the minimum TTL among
    /// `answers` (floored to one second). A disabled cache, a non-`NOERROR`
    /// status, or an empty answer list are all no-ops — this cache only
    /// ever holds positive, TTL-bearing results; negative/error responses
    /// always re-query upstream.
    ///
    /// Eviction, once `max_entries` is reached, drops the oldest surviving
    /// entry (FIFO) to make room, before inserting the new one.
    pub fn insert(&self, name: &str, record_type: &str, status: i32, answers: &[DnsRecord]) {
        if !self.enabled || status != 0 {
            return;
        }
        let Some(ttl_secs) = min_positive_ttl(answers) else {
            return;
        };

        let key = CacheKey::new(name, record_type);
        let entry = CacheEntry {
            answer: CachedAnswer {
                status,
                answers: answers.to_vec(),
            },
            expires_at: Instant::now() + Duration::from_secs(ttl_secs),
        };

        let mut state = self.lock_state();
        let already_present = state.entries.contains_key(&key);
        let at_capacity = state.entries.len() >= self.max_entries;
        if !already_present
            && at_capacity
            && let Some(oldest) = state.order.pop_front()
        {
            state.entries.remove(&oldest);
        }
        if state.entries.insert(key.clone(), entry).is_none() {
            state.order.push_back(key);
        }
    }

    /// A snapshot of current entry count plus cumulative hit/miss counters.
    /// `flush` clears entries but intentionally leaves the counters alone —
    /// they are a running activity log, not a property of the current
    /// entry set.
    pub fn stats(&self) -> CacheStats {
        let entries = self.lock_state().entries.len();
        CacheStats {
            entries,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }

    /// Empties every cached entry immediately.
    pub fn flush(&self) {
        let mut state = self.lock_state();
        state.entries.clear();
        state.order.clear();
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, CacheState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The minimum TTL among `answers`, each floored to at least one second —
/// `None` when `answers` is empty (nothing to cache: an empty positive
/// response is a NODATA case this cache deliberately never remembers).
fn min_positive_ttl(answers: &[DnsRecord]) -> Option<u64> {
    let mut min_ttl: Option<u64> = None;
    for answer in answers {
        let ttl = (answer.ttl.max(0) as u64).max(1);
        let Some(current) = min_ttl else {
            min_ttl = Some(ttl);
            continue;
        };
        if ttl < current {
            min_ttl = Some(ttl);
        }
    }
    min_ttl
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doh::RecordKind;

    fn answer(name: &str, ttl: i64, data: &str) -> DnsRecord {
        DnsRecord {
            name: name.to_string(),
            kind: RecordKind::Text("A".to_string()),
            ttl,
            data: data.to_string(),
        }
    }

    #[test]
    fn miss_on_empty_cache() {
        let cache = Cache::new(CacheConfig::default());
        assert!(cache.get("example.com.", "A").is_none());
        assert_eq!(cache.stats().misses, 1);
    }

    #[test]
    fn hit_after_insert_avoids_a_second_upstream_call() {
        let cache = Cache::new(CacheConfig::default());
        let answers = vec![answer("example.com.", 300, "192.0.2.1")];
        cache.insert("example.com.", "A", 0, &answers);

        let hit = cache.get("example.com.", "A");
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().answers[0].data, "192.0.2.1");

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 0);
        assert_eq!(stats.entries, 1);
    }

    #[test]
    fn lookup_is_case_insensitive_on_name_and_type() {
        let cache = Cache::new(CacheConfig::default());
        let answers = vec![answer("Example.COM.", 300, "192.0.2.1")];
        cache.insert("Example.COM.", "a", 0, &answers);

        assert!(cache.get("example.com.", "A").is_some());
    }

    #[test]
    fn entry_expires_after_its_ttl() {
        let cache = Cache::new(CacheConfig::default());
        let answers = vec![answer("example.com.", 1, "192.0.2.1")];
        cache.insert("example.com.", "A", 0, &answers);
        assert!(cache.get("example.com.", "A").is_some());

        std::thread::sleep(Duration::from_millis(1100));
        assert!(cache.get("example.com.", "A").is_none());
        assert_eq!(
            cache.stats().entries,
            0,
            "expired entry must be evicted on read"
        );
    }

    #[test]
    fn bounded_size_evicts_the_oldest_entry() {
        let cache = Cache::new(CacheConfig {
            enabled: true,
            max_entries: 2,
        });
        cache.insert(
            "a.example.",
            "A",
            0,
            &[answer("a.example.", 300, "192.0.2.1")],
        );
        cache.insert(
            "b.example.",
            "A",
            0,
            &[answer("b.example.", 300, "192.0.2.2")],
        );
        cache.insert(
            "c.example.",
            "A",
            0,
            &[answer("c.example.", 300, "192.0.2.3")],
        );

        assert_eq!(cache.stats().entries, 2);
        assert!(
            cache.get("a.example.", "A").is_none(),
            "oldest entry should have been evicted"
        );
        assert!(cache.get("b.example.", "A").is_some());
        assert!(cache.get("c.example.", "A").is_some());
    }

    #[test]
    fn flush_empties_entries_but_keeps_counters() {
        let cache = Cache::new(CacheConfig::default());
        cache.insert(
            "a.example.",
            "A",
            0,
            &[answer("a.example.", 300, "192.0.2.1")],
        );
        let _ = cache.get("a.example.", "A");
        let _ = cache.get("missing.example.", "A");

        cache.flush();

        let stats = cache.stats();
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert!(cache.get("a.example.", "A").is_none());
    }

    #[test]
    fn disabled_cache_always_misses() {
        let cache = Cache::new(CacheConfig {
            enabled: false,
            max_entries: 100,
        });
        cache.insert(
            "a.example.",
            "A",
            0,
            &[answer("a.example.", 300, "192.0.2.1")],
        );

        assert!(cache.get("a.example.", "A").is_none());
        assert!(cache.get("a.example.", "A").is_none());
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(cache.stats().misses, 2);
    }

    #[test]
    fn non_noerror_status_is_never_cached() {
        let cache = Cache::new(CacheConfig::default());
        cache.insert(
            "a.example.",
            "A",
            3,
            &[answer("a.example.", 300, "192.0.2.1")],
        );
        assert_eq!(cache.stats().entries, 0);
    }

    #[test]
    fn empty_answer_list_is_never_cached() {
        let cache = Cache::new(CacheConfig::default());
        cache.insert("a.example.", "A", 0, &[]);
        assert_eq!(cache.stats().entries, 0);
    }
}
