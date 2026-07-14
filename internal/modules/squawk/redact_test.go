package squawk

import (
	"context"
	"strings"
	"testing"

	"go.uber.org/zap/zaptest"
)

// TestConfigShowRedactsAuthToken guards a secret-leak regression: `penguin
// squawk config show` must never print the DoH auth token, which lands on
// terminals, in screenshots, and in support tickets.
func TestConfigShowRedactsAuthToken(t *testing.T) {
	const token = "supersecrettoken1234"

	host := NewFakeHost(zaptest.NewLogger(t), t.TempDir())
	host.secrets.store["auth_token"] = []byte(token)

	m := New().(*Module)
	if err := m.Init(context.Background(), host); err != nil {
		t.Fatalf("Init: %v", err)
	}

	res, err := m.handleConfig(context.Background(), nil)
	if err != nil {
		t.Fatalf("handleConfig: %v", err)
	}

	if strings.Contains(res.Output, token) {
		t.Fatal("config show leaked the auth token in Output")
	}
	if strings.Contains(string(res.JSON), token) {
		t.Fatal("config show leaked the auth token in JSON")
	}
	if !strings.Contains(res.Output, "****1234") {
		t.Errorf("expected a masked token hint, got:\n%s", res.Output)
	}
}

func TestMaskSecret(t *testing.T) {
	for _, tc := range []struct{ in, want string }{
		{"", ""},
		{"ab", "****"},
		{"abcd", "****"},
		{"abcdefgh", "****efgh"},
	} {
		if got := maskSecret(tc.in); got != tc.want {
			t.Errorf("maskSecret(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}
