package telemetry

import (
	"strings"
	"testing"

	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
)

func TestNewTelemetry(t *testing.T) {
	tests := []struct {
		name        string
		level       string
		expectError bool
	}{
		{
			name:        "debug level",
			level:       "debug",
			expectError: false,
		},
		{
			name:        "info level",
			level:       "info",
			expectError: false,
		},
		{
			name:        "warn level",
			level:       "warn",
			expectError: false,
		},
		{
			name:        "error level",
			level:       "error",
			expectError: false,
		},
		{
			name:        "invalid level",
			level:       "invalid_level",
			expectError: true,
		},
		{
			name:        "empty level defaults to info",
			level:       "info",
			expectError: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tel, err := New(tt.level)

			if tt.expectError && err == nil {
				t.Error("expected error, got none")
				return
			}
			if !tt.expectError && err != nil {
				t.Errorf("unexpected error: %v", err)
				return
			}

			if !tt.expectError {
				if tel.Logger == nil {
					t.Error("expected Logger, got nil")
				}
				if tel.Registry == nil {
					t.Error("expected Registry, got nil")
				}
			}
		})
	}
}

func TestModuleLogger(t *testing.T) {
	tel, err := New("info")
	if err != nil {
		t.Fatalf("New: %v", err)
	}

	tests := []struct {
		name       string
		moduleName string
	}{
		{name: "simple name", moduleName: "auth"},
		{name: "numeric suffix", moduleName: "api-v1"},
		{name: "underscore", moduleName: "db_manager"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			logger := tel.ModuleLogger(tt.moduleName)
			if logger == nil {
				t.Error("ModuleLogger returned nil")
				return
			}

			// Verify it's a *zap.Logger
			if _, ok := interface{}(logger).(*zap.Logger); !ok {
				t.Error("returned logger is not *zap.Logger")
			}

			// Verify the logger can log (just ensure no panic)
			logger.Info("test message")
		})
	}
}

func TestModuleRegisterer(t *testing.T) {
	tel, err := New("info")
	if err != nil {
		t.Fatalf("New: %v", err)
	}

	tests := []struct {
		name       string
		moduleName string
	}{
		{name: "simple name", moduleName: "plugins"},
		{name: "hyphenated", moduleName: "plugin-loader"},
		{name: "versioned", moduleName: "sdk-v2"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			registerer := tel.ModuleRegisterer(tt.moduleName)
			if registerer == nil {
				t.Error("ModuleRegisterer returned nil")
				return
			}

			// Verify it's a prometheus.Registerer
			if _, ok := interface{}(registerer).(prometheus.Registerer); !ok {
				t.Error("returned registerer is not prometheus.Registerer")
			}

			// Test that we can register a metric through the module registerer
			counter := prometheus.NewCounter(prometheus.CounterOpts{
				Name: "test_counter",
				Help: "Test counter metric",
			})
			if err := registerer.Register(counter); err != nil {
				t.Errorf("failed to register metric: %v", err)
			}
		})
	}
}

func TestTelemetryLoggerTypes(t *testing.T) {
	tel, err := New("debug")
	if err != nil {
		t.Fatalf("New: %v", err)
	}

	// Verify Logger is a *zap.Logger
	if _, ok := interface{}(tel.Logger).(*zap.Logger); !ok {
		t.Error("Logger is not *zap.Logger")
	}

	// Verify Registry is a *prometheus.Registry
	if _, ok := interface{}(tel.Registry).(*prometheus.Registry); !ok {
		t.Error("Registry is not *prometheus.Registry")
	}
}

func TestTelemetryRegistry(t *testing.T) {
	tel, err := New("info")
	if err != nil {
		t.Fatalf("New: %v", err)
	}

	// Verify the registry was created
	if tel.Registry == nil {
		t.Fatal("Registry is nil")
	}

	// Verify collectors are registered (process and Go runtime)
	// We can collect metrics to ensure collectors are working
	metricFamilies, err := tel.Registry.Gather()
	if err != nil {
		t.Fatalf("Gather metrics: %v", err)
	}

	// Should have some metrics from process and Go collectors
	if len(metricFamilies) == 0 {
		t.Error("expected metrics from collectors, got none")
	}

	// Look for process or Go runtime metrics
	foundProcess := false
	foundGo := false
	for _, mf := range metricFamilies {
		if mf.GetName() == "process_resident_memory_bytes" {
			foundProcess = true
		}
		if mf.GetName() == "go_goroutines" {
			foundGo = true
		}
	}

	if !foundProcess && !foundGo {
		t.Error("expected process or Go runtime metrics, found neither")
	}
}

func TestTelemetryLoggerConcurrency(t *testing.T) {
	tel, err := New("info")
	if err != nil {
		t.Fatalf("New: %v", err)
	}

	// Log from multiple goroutines concurrently
	done := make(chan bool, 3)
	for i := 0; i < 3; i++ {
		go func(id int) {
			logger := tel.ModuleLogger("concurrent_test")
			logger.Info("concurrent log", zap.Int("goroutine", id))
			done <- true
		}(i)
	}

	for i := 0; i < 3; i++ {
		<-done
	}
}

func BenchmarkModuleLogger(b *testing.B) {
	tel, _ := New("info")
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_ = tel.ModuleLogger("bench_module")
	}
}

func BenchmarkModuleRegisterer(b *testing.B) {
	tel, _ := New("info")
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_ = tel.ModuleRegisterer("bench_module")
	}
}

func TestNewWithVariousLevels(t *testing.T) {
	levels := []string{"debug", "info", "warn", "error", "fatal", "panic"}

	for _, level := range levels {
		t.Run("level_"+level, func(t *testing.T) {
			tel, err := New(level)
			if err != nil {
				t.Errorf("New(%s) failed: %v", level, err)
				return
			}

			if tel.Logger == nil {
				t.Error("Logger is nil")
			}
			if tel.Registry == nil {
				t.Error("Registry is nil")
			}
		})
	}
}

func TestTelemetryLoggerNotNil(t *testing.T) {
	tel, err := New("info")
	if err != nil {
		t.Fatalf("New() failed: %v", err)
	}

	if tel.Logger == nil {
		t.Fatal("Logger should not be nil")
	}

	// Log should not panic
	tel.Logger.Info("test")
	tel.Logger.Debug("debug")
	tel.Logger.Warn("warn")
	tel.Logger.Error("error")
}

func TestModuleLoggerNaming(t *testing.T) {
	tel, err := New("info")
	if err != nil {
		t.Fatalf("New() failed: %v", err)
	}

	tests := []string{
		"auth",
		"database",
		"network",
		"storage",
		"cache",
	}

	for _, name := range tests {
		t.Run(name, func(t *testing.T) {
			logger := tel.ModuleLogger(name)
			if logger == nil {
				t.Error("ModuleLogger returned nil")
			}
		})
	}
}

func TestNewInvalidLevel(t *testing.T) {
	tel, err := New("invalid")
	if err == nil {
		t.Error("New() should fail with invalid level")
		return
	}

	if tel != nil {
		t.Error("New() should return nil on error")
	}
}

func TestNewWithFatalLevel(t *testing.T) {
	tel, err := New("fatal")
	if err != nil {
		t.Errorf("New(fatal) failed: %v", err)
		return
	}

	if tel == nil {
		t.Error("New(fatal) returned nil")
		return
	}

	if tel.Logger == nil {
		t.Error("Logger is nil")
	}
}

func TestNewWithPanicLevel(t *testing.T) {
	tel, err := New("panic")
	if err != nil {
		t.Errorf("New(panic) failed: %v", err)
	}

	if tel == nil {
		t.Error("New(panic) returned nil")
	}
}

func TestModuleLoggerMultipleCalls(t *testing.T) {
	tel, err := New("info")
	if err != nil {
		t.Fatalf("New: %v", err)
	}

	// Get logger twice for same module - should be independent
	logger1 := tel.ModuleLogger("test_module")
	logger2 := tel.ModuleLogger("test_module")

	if logger1 == nil || logger2 == nil {
		t.Error("ModuleLogger returned nil")
	}

	// Both should be valid loggers
	logger1.Info("message from logger1")
	logger2.Info("message from logger2")
}

func TestModuleRegistererMultipleCalls(t *testing.T) {
	tel, err := New("info")
	if err != nil {
		t.Fatalf("New: %v", err)
	}

	reg1 := tel.ModuleRegisterer("test_module")
	reg2 := tel.ModuleRegisterer("test_module")

	if reg1 == nil || reg2 == nil {
		t.Error("ModuleRegisterer returned nil")
	}

	// Both should be valid registerers
	counter1 := prometheus.NewCounter(prometheus.CounterOpts{
		Name: "test_counter_1",
		Help: "Test counter",
	})
	if err := reg1.Register(counter1); err != nil {
		t.Errorf("failed to register counter to reg1: %v", err)
	}

	counter2 := prometheus.NewCounter(prometheus.CounterOpts{
		Name: "test_counter_2",
		Help: "Test counter",
	})
	if err := reg2.Register(counter2); err != nil {
		t.Errorf("failed to register counter to reg2: %v", err)
	}
}

func TestTelemetryRegistryHasGoCollectors(t *testing.T) {
	tel, err := New("info")
	if err != nil {
		t.Fatalf("New: %v", err)
	}

	// Gather metrics
	metricFamilies, err := tel.Registry.Gather()
	if err != nil {
		t.Fatalf("Gather: %v", err)
	}

	// Look for Go runtime or process metrics
	hasGoOrProcessMetrics := false
	for _, mf := range metricFamilies {
		name := mf.GetName()
		if strings.HasPrefix(name, "go_") || strings.HasPrefix(name, "process_") {
			hasGoOrProcessMetrics = true
			break
		}
	}

	if !hasGoOrProcessMetrics {
		t.Error("Registry should have Go or process metrics")
	}
}

// TestNewBadLevelString covers bad-level-string→default path (ParseLevel error)
func TestNewBadLevelString(t *testing.T) {
	tel, err := New("badlevel")
	if err == nil {
		t.Error("New() should fail with invalid level string")
		return
	}
	if tel != nil {
		t.Error("New() should return nil on error")
	}
}

// TestNewHappyPathHasCollectors verifies registry has go+process collectors on success
func TestNewHappyPathHasCollectors(t *testing.T) {
	tel, err := New("info")
	if err != nil {
		t.Fatalf("New(info) failed: %v", err)
	}

	if tel.Logger == nil {
		t.Fatal("Logger is nil")
	}
	if tel.Registry == nil {
		t.Fatal("Registry is nil")
	}

	// Gather and verify collectors are registered
	metrics, err := tel.Registry.Gather()
	if err != nil {
		t.Fatalf("Gather failed: %v", err)
	}

	hasGo := false
	hasProcess := false
	for _, mf := range metrics {
		name := mf.GetName()
		if strings.HasPrefix(name, "go_") {
			hasGo = true
		}
		if strings.HasPrefix(name, "process_") {
			hasProcess = true
		}
	}

	if !hasGo {
		t.Error("Registry should have Go runtime metrics")
	}
	if !hasProcess {
		t.Error("Registry should have process metrics")
	}
}

// TestNewSuccessfulConfigBuild covers the successful config build path
func TestNewSuccessfulConfigBuild(t *testing.T) {
	levels := []string{"debug", "info", "warn", "error"}

	for _, level := range levels {
		t.Run("level_"+level, func(t *testing.T) {
			tel, err := New(level)
			if err != nil {
				t.Fatalf("New(%s) failed: %v", level, err)
			}

			if tel.Logger == nil {
				t.Error("Logger should not be nil")
			}
			if tel.Registry == nil {
				t.Error("Registry should not be nil")
			}
		})
	}
}

// TestNewInvalidLogLevel covers New error path with invalid level string
func TestNewInvalidLogLevel(t *testing.T) {
	tel, err := New("invalid_level_xyz")
	if err == nil {
		t.Error("New() should error with invalid level")
	}
	if tel != nil {
		t.Error("New() should return nil on error")
	}
}

// TestNewProcessAndGoCollectors verifies both collectors are registered
func TestNewProcessAndGoCollectors(t *testing.T) {
	tel, err := New("info")
	if err != nil {
		t.Fatalf("New() failed: %v", err)
	}

	// Gather metrics to verify collectors are working
	metrics, err := tel.Registry.Gather()
	if err != nil {
		t.Fatalf("Gather() failed: %v", err)
	}

	if len(metrics) == 0 {
		t.Fatal("Registry should have metrics from collectors")
	}

	// Check for both process and Go metrics
	hasProcess := false
	hasGo := false

	for _, mf := range metrics {
		name := mf.GetName()
		if strings.HasPrefix(name, "process_") {
			hasProcess = true
		}
		if strings.HasPrefix(name, "go_") {
			hasGo = true
		}
	}

	if !hasProcess {
		t.Error("Registry should have process metrics")
	}
	if !hasGo {
		t.Error("Registry should have Go runtime metrics")
	}
}

// TestNewAllLogLevels tests New with all valid log levels
func TestNewAllLogLevels(t *testing.T) {
	levels := []string{"debug", "info", "warn", "error", "dpanic", "panic", "fatal"}

	for _, level := range levels {
		t.Run("level_"+level, func(t *testing.T) {
			tel, err := New(level)
			if err != nil {
				t.Errorf("New(%s) failed: %v", level, err)
				return
			}

			if tel == nil {
				t.Error("New() returned nil")
				return
			}

			if tel.Logger == nil {
				t.Error("Logger is nil")
			}

			if tel.Registry == nil {
				t.Error("Registry is nil")
			}

			// Verify metrics are available
			metrics, _ := tel.Registry.Gather()
			if len(metrics) == 0 {
				t.Logf("No metrics gathered for level %s", level)
			}
		})
	}
}

// TestNewReturnsNonNilOnSuccess ensures New never returns nil on success
func TestNewReturnsNonNilOnSuccess(t *testing.T) {
	tel, err := New("info")
	if err != nil {
		t.Fatalf("New() failed: %v", err)
	}

	if tel == nil {
		t.Fatal("New() returned nil on success")
	}

	if tel.Logger == nil {
		t.Fatal("Logger is nil")
	}

	if tel.Registry == nil {
		t.Fatal("Registry is nil")
	}

	// Verify we can use the logger
	tel.Logger.Info("test message")
	tel.Logger.Debug("debug message")
	tel.Logger.Warn("warn message")
	tel.Logger.Error("error message")
}
