package registry

import (
	"strings"
	"testing"
)

// TestAllMatchesBuiltins guards the contract that All() exposes exactly the
// registered factories.
func TestAllMatchesBuiltins(t *testing.T) {
	if got, want := len(All()), len(Builtins); got != want {
		t.Fatalf("All() returned %d factories, Builtins has %d", got, want)
	}
}

// TestRegisteredModulesAreWellFormed is the guard rail for the project's
// central extensibility promise: adding a product is one factory line here.
// Every registered factory must produce a usable module with a unique,
// CLI-safe name, a version, and (per house rules) a feature-flag key that
// follows the penguin.<module> convention.
func TestRegisteredModulesAreWellFormed(t *testing.T) {
	if len(Builtins) == 0 {
		t.Skip("no built-in modules registered yet")
	}

	seen := make(map[string]bool, len(Builtins))
	for i, factory := range Builtins {
		if factory == nil {
			t.Errorf("Builtins[%d] is nil", i)
			continue
		}

		m := factory()
		if m == nil {
			t.Errorf("Builtins[%d] factory returned nil", i)
			continue
		}

		info := m.Info()
		switch {
		case info.Name == "":
			t.Errorf("Builtins[%d]: empty Info().Name", i)
		case strings.ContainsAny(info.Name, " /\\.."):
			// The name becomes `penguin <name> ...` and a config file path.
			t.Errorf("Builtins[%d]: name %q must not contain spaces or path separators", i, info.Name)
		case seen[info.Name]:
			t.Errorf("Builtins[%d]: duplicate module name %q", i, info.Name)
		}
		seen[info.Name] = true

		if info.Version == "" {
			t.Errorf("module %q: empty Info().Version", info.Name)
		}
		if f := info.LicenseFeature; f != "" && !strings.HasPrefix(f, "penguin.") {
			t.Errorf("module %q: LicenseFeature %q must use the penguin.<feature> convention", info.Name, f)
		}
	}
}

// TestFactoriesReturnFreshInstances ensures a factory is not handing back a
// shared singleton — the supervisor builds a new instance on every restart.
func TestFactoriesReturnFreshInstances(t *testing.T) {
	for _, factory := range Builtins {
		if factory == nil {
			continue
		}
		if a, b := factory(), factory(); a == b {
			t.Errorf("factory for %q returned the same instance twice", a.Info().Name)
		}
	}
}
