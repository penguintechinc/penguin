package daemon

import (
	"os"
	"path/filepath"
	"testing"
)

const testSchema = `{
  "type": "object",
  "properties": {
    "doh": {
      "type": "object",
      "properties": {
        "server_url": {"type": "string"},
        "verify_tls": {"type": "boolean"}
      }
    }
  }
}`

func writeModuleConfig(t *testing.T, dir, name, body string) {
	t.Helper()
	mdir := filepath.Join(dir, "modules.d")
	if err := os.MkdirAll(mdir, 0o750); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(mdir, name+".yaml"), []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}
}

// TestModuleRawValid returns the file verbatim once it satisfies the schema.
func TestModuleRawValid(t *testing.T) {
	dir := t.TempDir()
	body := "doh:\n  server_url: \"https://dns.example/dns-query\"\n  verify_tls: true\n"
	writeModuleConfig(t, dir, "squawk", body)

	raw, err := NewConfigStore(dir).ModuleRaw("squawk", []byte(testSchema))
	if err != nil {
		t.Fatalf("ModuleRaw: %v", err)
	}
	if string(raw) != body {
		t.Errorf("ModuleRaw returned %q, want the file verbatim", raw)
	}
}

// TestModuleRawSchemaViolation is the guarantee that a module never receives
// config the daemon has not validated.
func TestModuleRawSchemaViolation(t *testing.T) {
	dir := t.TempDir()
	writeModuleConfig(t, dir, "squawk", "doh:\n  verify_tls: \"yes-please\"\n") // string, not bool

	if _, err := NewConfigStore(dir).ModuleRaw("squawk", []byte(testSchema)); err == nil {
		t.Fatal("expected schema violation to be rejected, got nil error")
	}
}

// TestModuleRawMissingFile yields no bytes and no error: defaults apply.
func TestModuleRawMissingFile(t *testing.T) {
	raw, err := NewConfigStore(t.TempDir()).ModuleRaw("squawk", []byte(testSchema))
	if err != nil {
		t.Fatalf("missing config should not error: %v", err)
	}
	if raw != nil {
		t.Errorf("expected nil bytes for missing config, got %q", raw)
	}
}

// TestModuleRawPathTraversal rejects names that escape modules.d.
func TestModuleRawPathTraversal(t *testing.T) {
	for _, name := range []string{"../secrets", "a/b", `a\b`} {
		if _, err := NewConfigStore(t.TempDir()).ModuleRaw(name, nil); err == nil {
			t.Errorf("expected %q to be rejected as a path traversal", name)
		}
	}
}
