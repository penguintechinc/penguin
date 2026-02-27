package waddleperf

import (
	"context"
	"sync"
	"time"

	"github.com/sirupsen/logrus"
)

// TestRunner manages scheduled network test execution.
type TestRunner struct {
	client   *Client
	schedule ScheduleConfig
	logger   *logrus.Logger

	mu     sync.Mutex
	stopCh chan struct{}
	wg     sync.WaitGroup
}

// NewTestRunner creates a test runner with the given schedule configuration.
func NewTestRunner(client *Client, schedule ScheduleConfig, logger *logrus.Logger) *TestRunner {
	return &TestRunner{
		client:   client,
		schedule: schedule,
		logger:   logger,
		stopCh:   make(chan struct{}),
	}
}

// Start begins periodic test execution if the schedule is enabled.
func (r *TestRunner) Start() {
	r.mu.Lock()
	defer r.mu.Unlock()
	if !r.schedule.Enabled || len(r.schedule.Tests) == 0 {
		return
	}
	r.wg.Add(1)
	go r.scheduleLoop()
}

// Stop halts the scheduler and waits for any in-flight tests to complete.
func (r *TestRunner) Stop() {
	r.mu.Lock()
	defer r.mu.Unlock()
	close(r.stopCh)
	r.wg.Wait()
}

// RunAll executes all configured tests and returns results.
func (r *TestRunner) RunAll(ctx context.Context, configs []TestConfig) []TestResult {
	var results []TestResult
	for _, cfg := range configs {
		result := r.RunOnce(ctx, cfg)
		results = append(results, result)
	}
	return results
}

// RunOnce executes a single test and uploads the result.
func (r *TestRunner) RunOnce(ctx context.Context, cfg TestConfig) TestResult {
	var result *TestResult
	var err error

	switch cfg.Type {
	case TestHTTP:
		result, err = r.client.RunHTTPTest(ctx, cfg.Target, cfg.Protocol)
	case TestTCP:
		result, err = r.client.RunTCPTest(ctx, cfg.Target, cfg.Protocol)
	case TestUDP:
		result, err = r.client.RunUDPTest(ctx, cfg.Target, cfg.Protocol)
	case TestICMP:
		result, err = r.client.RunICMPTest(ctx, cfg.Target, cfg.Protocol)
	default:
		r.logger.WithField("type", cfg.Type).Warn("unknown test type")
		return TestResult{
			Type:      cfg.Type,
			Target:    cfg.Target,
			Status:    "failed",
			Timestamp: time.Now().UTC().Format(time.RFC3339),
		}
	}

	if err != nil {
		r.logger.WithError(err).WithField("type", cfg.Type).Warn("test execution failed")
		return TestResult{
			Type:      cfg.Type,
			Target:    cfg.Target,
			Status:    "failed",
			Timestamp: time.Now().UTC().Format(time.RFC3339),
		}
	}

	// Upload result to manager server.
	uploadCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if uploadErr := r.client.UploadResult(uploadCtx, *result); uploadErr != nil {
		r.logger.WithError(uploadErr).Warn("result upload failed")
	}

	return *result
}

func (r *TestRunner) scheduleLoop() {
	defer r.wg.Done()
	interval := time.Duration(r.schedule.Interval) * time.Second
	if interval < time.Second {
		interval = 60 * time.Second
	}
	ticker := time.NewTicker(interval)
	defer ticker.Stop()

	for {
		select {
		case <-r.stopCh:
			return
		case <-ticker.C:
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
			r.RunAll(ctx, r.schedule.Tests)
			cancel()
		}
	}
}
