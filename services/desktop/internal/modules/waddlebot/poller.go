package waddlebot

import (
	"context"
	"sync"
	"time"

	desktop "github.com/penguintechinc/penguin-libs/packages/penguin-desktop"
	"github.com/sirupsen/logrus"
)

// Poller drives the periodic action-fetch loop and keepalive heartbeat.
// It delegates to a Client for HTTP communication and an ActionHandler for
// dispatching received actions to OBS, scripting, or other subsystems.
type Poller struct {
	client      *Client
	actions     *ActionHandler
	logger      *logrus.Logger
	pollWorker  *desktop.TickWorker
	heartbeat   *desktop.TickWorker
	mu          sync.RWMutex
	pollCount   int
	lastPoll    time.Time
}

// NewPoller creates a Poller that ticks at pollInterval.
// A 60-second heartbeat TickWorker is also configured automatically.
func NewPoller(client *Client, actions *ActionHandler, logger *logrus.Logger, pollInterval time.Duration) *Poller {
	p := &Poller{
		client:  client,
		actions: actions,
		logger:  logger,
	}

	p.pollWorker = &desktop.TickWorker{
		Interval: pollInterval,
		Timeout:  30 * time.Second,
		Action:   p.poll,
		OnError:  func(err error) { logger.WithError(err).Warn("waddlebot: poll failed") },
	}

	p.heartbeat = &desktop.TickWorker{
		Interval: 60 * time.Second,
		Timeout:  10 * time.Second,
		Action:   client.Heartbeat,
		OnError:  func(err error) { logger.WithError(err).Warn("waddlebot: heartbeat failed") },
	}

	return p
}

// Start begins the poll loop and heartbeat goroutines.
func (p *Poller) Start() {
	p.pollWorker.Start()
	p.heartbeat.Start()
	p.logger.Info("waddlebot: poller started")
}

// Stop signals both workers to exit and waits for them to finish.
func (p *Poller) Stop() {
	p.pollWorker.Stop()
	p.heartbeat.Stop()
	p.logger.Info("waddlebot: poller stopped")
}

// GetStats returns a snapshot of poll activity metrics.
func (p *Poller) GetStats() map[string]interface{} {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return map[string]interface{}{
		"poll_count": p.pollCount,
		"last_poll":  p.lastPoll,
	}
}

// poll is the TickWorker action: fetches pending actions and dispatches them.
func (p *Poller) poll(ctx context.Context) error {
	resp, err := p.client.Poll(ctx)
	if err != nil {
		return err
	}

	p.mu.Lock()
	p.pollCount++
	p.lastPoll = time.Now()
	p.mu.Unlock()

	if len(resp.Actions) == 0 {
		return nil
	}

	p.logger.WithField("count", len(resp.Actions)).Debug("waddlebot: received actions")

	for _, action := range resp.Actions {
		if time.Now().After(action.ExpiresAt) {
			p.logger.WithField("action_id", action.ID).Warn("waddlebot: skipping expired action")
			continue
		}
		go p.processAction(ctx, action)
	}

	return nil
}

// processAction executes a single action and reports the result to the server.
func (p *Poller) processAction(ctx context.Context, action ActionRequest) {
	timeout := time.Duration(action.Timeout) * time.Second
	if timeout <= 0 {
		timeout = 30 * time.Second
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	start := time.Now()
	result, err := p.actions.Execute(ctx, action)
	duration := time.Since(start).Milliseconds()

	aresp := ActionResponse{
		ID:       action.ID,
		Duration: duration,
	}
	if err != nil {
		aresp.Success = false
		aresp.Error = err.Error()
		p.logger.WithError(err).WithField("action", action.Action).Warn("waddlebot: action failed")
	} else {
		aresp.Success = true
		aresp.Result = result
		p.logger.WithFields(logrus.Fields{
			"action":   action.Action,
			"duration": duration,
		}).Debug("waddlebot: action succeeded")
	}

	if sendErr := p.client.SendResponse(ctx, aresp); sendErr != nil {
		p.logger.WithError(sendErr).Warn("waddlebot: failed to send action response")
	}
}
