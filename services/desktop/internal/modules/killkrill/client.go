package killkrill

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/sirupsen/logrus"
)

// Client communicates with the KillKrill logging/metrics service.
type Client struct {
	baseURL      string
	clientID     string
	clientSecret string
	httpClient   *http.Client
	logger       *logrus.Logger

	mu           sync.Mutex
	accessToken  string
	refreshToken string
	logQueue     []LogEntry
	metricQueue  []MetricEntry
	lastFlush    time.Time
	connected    bool

	stopCh chan struct{}
	wg     sync.WaitGroup
}

// NewClient creates a KillKrill API client.
func NewClient(baseURL, clientID, clientSecret string, logger *logrus.Logger) *Client {
	return &Client{
		baseURL:      strings.TrimRight(baseURL, "/"),
		clientID:     clientID,
		clientSecret: clientSecret,
		httpClient:   &http.Client{Timeout: 30 * time.Second},
		logger:       logger,
		stopCh:       make(chan struct{}),
	}
}

// Connect authenticates and starts the background flush worker.
func (c *Client) Connect(ctx context.Context) error {
	if err := c.authenticate(ctx); err != nil {
		return fmt.Errorf("killkrill auth: %w", err)
	}
	c.mu.Lock()
	c.connected = true
	c.mu.Unlock()

	c.wg.Add(1)
	go c.flushWorker()
	return nil
}

// Disconnect stops the flush worker and flushes remaining items.
func (c *Client) Disconnect(ctx context.Context) {
	close(c.stopCh)
	c.wg.Wait()

	// Final flush of remaining items.
	if err := c.Flush(ctx); err != nil {
		c.logger.WithError(err).Warn("final killkrill flush failed")
	}
	c.mu.Lock()
	c.connected = false
	c.mu.Unlock()
}

// SubmitLog queues a log entry for batch submission.
func (c *Client) SubmitLog(entry LogEntry) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.logQueue = append(c.logQueue, entry)
}

// SubmitMetric queues a metric entry for batch submission.
func (c *Client) SubmitMetric(entry MetricEntry) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.metricQueue = append(c.metricQueue, entry)
}

// Flush sends all queued logs and metrics to KillKrill.
func (c *Client) Flush(ctx context.Context) error {
	c.mu.Lock()
	logs := c.logQueue
	metrics := c.metricQueue
	c.logQueue = nil
	c.metricQueue = nil
	c.mu.Unlock()

	var errs []string

	if len(logs) > 0 {
		if err := c.doJSON(ctx, "POST", c.baseURL+"/api/v1/logs", logs, nil); err != nil {
			errs = append(errs, fmt.Sprintf("logs: %v", err))
		}
	}

	if len(metrics) > 0 {
		if err := c.doJSON(ctx, "POST", c.baseURL+"/api/v1/metrics", metrics, nil); err != nil {
			errs = append(errs, fmt.Sprintf("metrics: %v", err))
		}
	}

	c.mu.Lock()
	c.lastFlush = time.Now()
	c.mu.Unlock()

	if len(errs) > 0 {
		return fmt.Errorf("flush errors: %s", strings.Join(errs, "; "))
	}
	return nil
}

// HealthCheck verifies connectivity to the KillKrill service.
func (c *Client) HealthCheck(ctx context.Context) error {
	req, err := http.NewRequestWithContext(ctx, "GET", c.baseURL+"/health", nil)
	if err != nil {
		return err
	}
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 400 {
		return fmt.Errorf("health check returned status %d", resp.StatusCode)
	}
	return nil
}

// GetQueueStatus returns current queue depths and connection state.
func (c *Client) GetQueueStatus() QueueStatus {
	c.mu.Lock()
	defer c.mu.Unlock()
	flush := ""
	if !c.lastFlush.IsZero() {
		flush = c.lastFlush.Format(time.RFC3339)
	}
	return QueueStatus{
		LogsPending:    len(c.logQueue),
		MetricsPending: len(c.metricQueue),
		LastFlush:      flush,
		Connected:      c.connected,
	}
}

func (c *Client) authenticate(ctx context.Context) error {
	body := map[string]string{
		"client_id":     c.clientID,
		"client_secret": c.clientSecret,
	}
	var resp authResponse
	if err := c.doJSON(ctx, "POST", c.baseURL+"/api/v1/auth/login", body, &resp); err != nil {
		return err
	}
	c.mu.Lock()
	c.accessToken = resp.AccessToken
	c.refreshToken = resp.RefreshToken
	c.mu.Unlock()
	return nil
}

func (c *Client) flushWorker() {
	defer c.wg.Done()
	ticker := time.NewTicker(5 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-c.stopCh:
			return
		case <-ticker.C:
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			if err := c.Flush(ctx); err != nil {
				c.logger.WithError(err).Warn("killkrill flush failed")
			}
			cancel()
		}
	}
}

func (c *Client) doJSON(ctx context.Context, method, url string, body, result interface{}) error {
	var bodyReader io.Reader
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			return fmt.Errorf("marshaling request: %w", err)
		}
		bodyReader = bytes.NewReader(data)
	}

	req, err := http.NewRequestWithContext(ctx, method, url, bodyReader)
	if err != nil {
		return fmt.Errorf("creating request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")

	c.mu.Lock()
	token := c.accessToken
	c.mu.Unlock()
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode >= 400 {
		respBody, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("API error (status %d): %s", resp.StatusCode, string(respBody))
	}

	if result != nil && resp.StatusCode != http.StatusNoContent {
		if err := json.NewDecoder(resp.Body).Decode(result); err != nil {
			return fmt.Errorf("decoding response: %w", err)
		}
	}
	return nil
}
