package main

import (
	"testing"

	"github.com/kardianos/service"
)

// TestHandleServiceCommandMissing tests that non-service args are not handled.
func TestHandleServiceCommandMissing(t *testing.T) {
	tests := []struct {
		name    string
		args    []string
		handled bool
	}{
		{
			name:    "no args",
			args:    []string{},
			handled: false,
		},
		{
			name:    "non-service command",
			args:    []string{"version"},
			handled: false,
		},
		{
			name:    "random arg",
			args:    []string{"foo"},
			handled: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			handled, err := handleServiceCommand(tt.args)
			if handled != tt.handled {
				t.Errorf("handled = %v, want %v", handled, tt.handled)
			}
			if err != nil {
				t.Errorf("unexpected error: %v", err)
			}
		})
	}
}

// TestHandleServiceCommandErrors tests service subcommand error cases.
func TestHandleServiceCommandErrors(t *testing.T) {
	tests := []struct {
		name      string
		args      []string
		wantError bool
	}{
		{
			name:      "service without action",
			args:      []string{"service"},
			wantError: true,
		},
		{
			name:      "service with invalid action",
			args:      []string{"service", "invalid"},
			wantError: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			handled, err := handleServiceCommand(tt.args)
			if !handled {
				t.Errorf("handled = false, want true")
			}
			if (err != nil) != tt.wantError {
				t.Errorf("error = %v, wantError %v", err, tt.wantError)
			}
		})
	}
}

// NOTE: valid actions (install/uninstall/start/stop/status) are intentionally
// NOT exercised here — handleServiceCommand runs the real kardianos
// service.Control(), which on a systemd host goes through polkit and prompts the
// developer for a password on every `go test`. Action dispatch is validated
// indirectly by the "invalid action" error case above; the real Control() path
// is covered by manual/integration use, never the unit suite.

// TestNullProgramImplementsService verifies the nullProgram implements service.Service.
func TestNullProgramImplementsService(t *testing.T) {
	// Verify the interface is correctly implemented by assigning to the service.Service interface
	var prog interface{} = (*nullProgram)(nil)
	if _, ok := prog.(interface {
		Start(service.Service) error
		Stop(service.Service) error
	}); !ok {
		t.Fatal("nullProgram does not implement service.Service interface")
	}
}
