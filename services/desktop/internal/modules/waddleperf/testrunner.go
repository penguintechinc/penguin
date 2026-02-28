package waddleperf

import (
	"context"
	"time"

	desktop "github.com/penguintechinc/penguin-libs/packages/penguin-desktop"
	"github.com/sirupsen/logrus"
)

// TestRunner manages scheduled network test execution.
type TestRunner struct {
	client         *Client
	schedule       ScheduleConfig
	logger         *logrus.Logger
	scheduleWorker *desktop.TickWorker
}

// NewTestRunner creates a test runner with the given schedule configuration.
func NewTestRunner(client *Client, schedule ScheduleConfig, logger *logrus.Logger) *TestRunner {
	return &TestRunner{
		client:   client,
		schedule: schedule,
		logger:   logger,
	}
}

// Start begins periodic test execution if the schedule is enabled.
func (r *TestRunner) Start() {
	if !r.schedule.Enabled || len(r.schedule.Tests) == 0 {
		return
	}

	interval := time.Duration(r.schedule.Interval) * time.Second
	if interval < time.Second {
		interval = 60 * time.Second
	}

	r.scheduleWorker = &desktop.TickWorker{
		Interval: interval,
		Timeout:  5 * time.Minute,
		Action:   r.runScheduledTests,
		OnError: func(err error) {
			r.logger.WithError(err).Warn("scheduled test failed")
		},
	}
	r.scheduleWorker.Start()
}

// Stop halts the scheduler and waits for any in-flight tests to complete.
func (r *TestRunner) Stop() {
	if r.scheduleWorker != nil {
		r.scheduleWorker.Stop()
	}
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

func (r *TestRunner) runScheduledTests(ctx context.Context) error {
	r.RunAll(ctx, r.schedule.Tests)
	return nil
}
