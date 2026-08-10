package daemon

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// corpusCase mirrors one JSON fixture in testdata/config-corpus.
type corpusCase struct {
	Description  string          `json:"description"`
	Valid        bool            `json:"valid"`
	Schema       json.RawMessage `json:"schema"`
	InstanceYaml string          `json:"instanceYaml"`
}

// TestConfigCorpusConformance runs the shared config corpus through the frozen
// Go config store. It is the oracle half of the M1 schema-parity gate: the Rust
// store (crates/penguin-daemon/tests/config_corpus.rs) must return identical
// accept/reject verdicts on the very same corpus. If the two engines ever
// diverge, one of these two tests fails.
func TestConfigCorpusConformance(t *testing.T) {
	// This package's test CWD is go-client/internal/daemon; the corpus lives at
	// the repo root, three levels up.
	dir := filepath.Join("..", "..", "..", "testdata", "config-corpus")
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatalf("read corpus dir %s: %v", dir, err)
	}

	checked := 0
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".json") {
			continue
		}

		path := filepath.Join(dir, entry.Name())
		raw, err := os.ReadFile(path)
		if err != nil {
			t.Fatalf("read %s: %v", path, err)
		}

		var c corpusCase
		if err := json.Unmarshal(raw, &c); err != nil {
			t.Fatalf("parse %s: %v", path, err)
		}

		tmp := t.TempDir()
		modules := filepath.Join(tmp, "modules.d")
		if err := os.MkdirAll(modules, 0o750); err != nil {
			t.Fatalf("mkdir modules.d: %v", err)
		}
		if err := os.WriteFile(filepath.Join(modules, "mod.yaml"), []byte(c.InstanceYaml), 0o600); err != nil {
			t.Fatalf("write instance: %v", err)
		}

		cs := NewConfigStore(tmp)
		_, err = cs.Module("mod", []byte(c.Schema))
		gotValid := err == nil
		if gotValid != c.Valid {
			t.Errorf("%s (%s): expected valid=%v, got valid=%v (err=%v)",
				entry.Name(), c.Description, c.Valid, gotValid, err)
		}
		checked++
	}

	if checked < 12 {
		t.Fatalf("expected the config corpus, only checked %d cases", checked)
	}
}
