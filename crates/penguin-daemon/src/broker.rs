//! The shared event broker: the single fan-out point for module status
//! events, serving both in-process publishers and `WatchEvents` gRPC
//! subscribers from the same instance.
//!
//! # Fixes the Go double-broker bug
//!
//! The Go daemon constructs two brokers: the host factory publishes module
//! events into one, while `WatchEvents` subscribes to a second broker that
//! nothing ever publishes to, so module events never reach a watcher (see
//! `go-client/internal/daemon/host.go` and `server.go`). Here a single
//! [`EventBroker`] is built once by the caller and handed to both the
//! per-module `HostServices::events` implementation and the gRPC service, so
//! publish and subscribe always share one broker.

use tokio::sync::broadcast;

use penguin_sdk::{Event, EventSink};

/// The receiving half of an event subscription. Only events published after
/// [`EventBroker::subscribe`] was called arrive on it; a subscriber that
/// falls too far behind has its oldest unread events dropped rather than
/// stalling the publisher.
pub type EventReceiver = broadcast::Receiver<Event>;

/// A bounded, multi-subscriber fan-out for [`Event`]s.
///
/// Backed by [`tokio::sync::broadcast`], which gives two properties this
/// broker relies on: a lagging subscriber loses its oldest unread events
/// instead of blocking the publisher, and publishing with zero subscribers is
/// a silent no-op rather than an error.
pub struct EventBroker {
    sender: broadcast::Sender<Event>,
}

impl EventBroker {
    /// Creates a broker whose per-subscriber ring holds up to `capacity`
    /// unread events before the slowest subscriber starts lagging.
    pub fn new(capacity: usize) -> EventBroker {
        let (sender, _receiver) = broadcast::channel(capacity);
        EventBroker { sender }
    }

    /// Publishes `event` to every current subscriber.
    ///
    /// Synchronous and non-blocking by design: the supervisor calls this
    /// while holding its own internal lock, so an `.await`-ing publish would
    /// serialise every supervisor operation behind however long the slowest
    /// subscriber takes to drain its channel. `broadcast::Sender::send` never
    /// blocks — it copies the event into each subscriber's ring, or, with no
    /// subscribers at all, returns an error that this function discards.
    pub fn publish(&self, event: Event) {
        let _ = self.sender.send(event);
    }

    /// Subscribes to future events. The returned receiver sees nothing
    /// published before this call.
    pub fn subscribe(&self) -> EventReceiver {
        self.sender.subscribe()
    }
}

/// Lets one [`EventBroker`] be handed to a module's `HostServices::events` so
/// module publishers and gRPC `WatchEvents` subscribers share the same
/// fan-out (see the module doc).
impl EventSink for EventBroker {
    fn publish(&self, event: Event) {
        EventBroker::publish(self, event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::SystemTime;

    /// Builds a minimal event with `message` set, for assertions that only
    /// care about which event arrived.
    fn sample_event(message: &str) -> Event {
        Event {
            module: "squawk".to_string(),
            event_type: penguin_sdk::EventType::Info,
            message: message.to_string(),
            at: SystemTime::now(),
            fields: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn one_subscriber_receives_a_published_event() {
        let broker = EventBroker::new(4);
        let mut sub = broker.subscribe();

        broker.publish(sample_event("hello"));

        assert_eq!(sub.recv().await.unwrap().message, "hello");
    }

    #[tokio::test]
    async fn two_subscribers_both_receive_the_same_event() {
        let broker = EventBroker::new(4);
        let mut sub1 = broker.subscribe();
        let mut sub2 = broker.subscribe();

        broker.publish(sample_event("fanout"));

        assert_eq!(sub1.recv().await.unwrap().message, "fanout");
        assert_eq!(sub2.recv().await.unwrap().message, "fanout");
    }

    #[tokio::test]
    async fn a_subscriber_that_subscribes_after_a_publish_does_not_receive_it() {
        let broker = EventBroker::new(4);
        broker.publish(sample_event("before"));

        let mut sub = broker.subscribe();
        broker.publish(sample_event("after"));

        // The only event this subscriber can ever see is "after" — "before"
        // was published before it existed.
        assert_eq!(sub.recv().await.unwrap().message, "after");
    }

    #[test]
    fn publish_with_no_subscribers_is_a_silent_success() {
        let broker = EventBroker::new(4);
        broker.publish(sample_event("into the void"));
    }

    #[tokio::test]
    async fn the_event_sink_impl_routes_into_the_same_broker() {
        let broker = EventBroker::new(4);
        let mut sub = broker.subscribe();

        let sink: &dyn EventSink = &broker;
        sink.publish(sample_event("via trait object"));

        assert_eq!(sub.recv().await.unwrap().message, "via trait object");
    }
}
