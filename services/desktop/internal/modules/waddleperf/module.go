package waddleperf

import (
	"context"
	"fmt"

	"github.com/penguintechinc/penguin/services/desktop/internal/module"
	"github.com/penguintechinc/penguin/services/desktop/pkg/clischema"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
	"github.com/penguintechinc/penguin/services/desktop/pkg/uischema"
	"github.com/sirupsen/logrus"
)

// Module implements the WaddlePerf network testing client as a PluginModule.
type Module struct {
	module.BaseModule
	client *Client
	runner *TestRunner
}

func New() *Module              { return &Module{} }
func (m *Module) Name() string        { return "waddleperf" }
func (m *Module) DisplayName() string { return "WaddlePerf" }
func (m *Module) Description() string { return "Network performance testing client for WaddlePerf" }
func (m *Module) Version() string     { return "0.1.0" }

func (m *Module) Init(ctx context.Context, deps module.Dependencies) error {
	m.Logger = deps.Logger.(*logrus.Logger)
	return nil
}

func (m *Module) Start(ctx context.Context) error {
	m.MarkStarted()
	if m.runner != nil {
		m.runner.Start()
	}
	m.Logger.Info("WaddlePerf module started")
	return nil
}

func (m *Module) Stop(ctx context.Context) error {
	if m.runner != nil {
		m.runner.Stop()
	}
	m.MarkStopped()
	return nil
}

func (m *Module) HealthCheck(ctx context.Context) module.HealthStatus {
	if !m.IsStarted() {
		return m.NotStartedStatus()
	}
	if m.client == nil {
		return module.HealthStatus{State: module.HealthDegraded, Message: "client not configured"}
	}
	testOK, managerOK := m.client.HealthCheck(ctx)
	details := map[string]string{
		"test_server":    fmt.Sprintf("%v", testOK),
		"manager_server": fmt.Sprintf("%v", managerOK),
	}
	if testOK && managerOK {
		return module.HealthStatus{State: module.HealthHealthy, Message: "both servers reachable", Details: details}
	}
	if testOK || managerOK {
		return module.HealthStatus{State: module.HealthDegraded, Message: "partial connectivity", Details: details}
	}
	return module.HealthStatus{State: module.HealthUnhealthy, Message: "both servers unreachable", Details: details}
}

// --- GUI ---

func (m *Module) GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error) {
	if m.client == nil {
		return uischema.Panel(
			uischema.Card("wp-card", "WaddlePerf", "Network Performance Testing",
				uischema.Label("wp-nocfg", "Not configured. Set waddleperf.test_server_url in config."),
			),
		), nil
	}

	testOK, managerOK := m.client.HealthCheck(ctx)
	connText := fmt.Sprintf("Test Server: %s | Manager: %s",
		boolStatus(testOK), boolStatus(managerOK))

	// Recent results summary.
	resultsText := "No recent results"
	results, err := m.client.GetRecentResults(ctx, 5)
	if err == nil && len(results) > 0 {
		resultsText = ""
		for _, r := range results {
			resultsText += fmt.Sprintf("%s %s → %.1fms (loss: %.1f%%)\n",
				r.Type, r.Target, r.Latency, r.PacketLoss)
		}
	}

	return uischema.Panel(
		uischema.VBox(
			uischema.Card("wp-conn-card", "WaddlePerf", "Network Performance Testing",
				uischema.Label("wp-conn", connText),
			),
			uischema.Card("wp-results-card", "Recent Results", "",
				uischema.Label("wp-results", resultsText),
			),
			uischema.Card("wp-run-card", "Run Test", "",
				uischema.Select("wp-type", []string{"http", "tcp", "udp", "icmp"}, "http"),
				uischema.Entry("wp-target", "Target (URL or host:port)"),
				uischema.Button("wp-run-btn", "Run Test"),
			),
		),
	), nil
}

func (m *Module) HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error) {
	// Test execution is handled through CLI; GUI returns updated panel.
	return m.GetGUIPanel(ctx)
}

// --- CLI ---

func (m *Module) GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error) {
	wpCmd := clischema.Command("waddleperf", "WaddlePerf network performance testing")

	testCmd := clischema.Command("test", "Run a network test")
	clischema.WithFlags(testCmd,
		clischema.Flag("type", "t", "Test type (http/tcp/udp/icmp)", "http"),
		clischema.Flag("target", "", "Target URL or host:port", ""),
		clischema.Flag("protocol", "p", "Protocol (auto/http1/http2/tls/raw/dns/ping)", "auto"),
	)

	resultsCmd := clischema.Command("results", "Show recent test results")
	clischema.WithFlags(resultsCmd,
		clischema.Flag("limit", "n", "Number of results to show", "10"),
	)

	clischema.WithSubcommands(wpCmd,
		*testCmd,
		*resultsCmd,
		*clischema.Command("status", "Show connection and schedule status"),
	)
	return clischema.CommandList(*wpCmd), nil
}

func (m *Module) ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	if m.client == nil {
		return modulepb.ErrorResponse(fmt.Errorf("waddleperf client not configured")), nil
	}

	switch req.CommandPath {
	case "waddleperf test":
		testType := TestType(req.Flags["type"])
		if testType == "" {
			testType = TestHTTP
		}
		target := req.Flags["target"]
		if target == "" && len(req.Args) > 0 {
			target = req.Args[0]
		}
		if target == "" {
			return modulepb.ErrorResponse(fmt.Errorf("target required (--target URL or host:port)")), nil
		}
		protocol := req.Flags["protocol"]
		if protocol == "" {
			protocol = "auto"
		}

		cfg := TestConfig{Type: testType, Target: target, Protocol: protocol}
		result := m.runner.RunOnce(ctx, cfg)

		out := fmt.Sprintf("Test:         %s\nTarget:       %s\nStatus:       %s\nLatency:      %.2f ms\nDNS Lookup:   %.2f ms\nTCP Connect:  %.2f ms\nTLS Handshake:%.2f ms\nTTFB:         %.2f ms\nTotal Time:   %.2f ms\nJitter:       %.2f ms\nPacket Loss:  %.2f%%\n",
			result.Type, result.Target, result.Status,
			result.Latency, result.DNSLookup, result.TCPConnect,
			result.TLSHandshake, result.TTFB, result.TotalTime,
			result.Jitter, result.PacketLoss)
		return modulepb.OKResponse(out), nil

	case "waddleperf results":
		limitStr := req.Flags["limit"]
		limit := 10
		if limitStr != "" {
			fmt.Sscanf(limitStr, "%d", &limit)
		}
		results, err := m.client.GetRecentResults(ctx, limit)
		if err != nil {
			return modulepb.ErrorResponse(err), nil
		}
		if len(results) == 0 {
			return modulepb.OKResponse("No recent results\n"), nil
		}
		out := fmt.Sprintf("%-6s %-30s %-8s %10s %8s %8s\n", "TYPE", "TARGET", "STATUS", "LATENCY", "JITTER", "LOSS")
		for _, r := range results {
			out += fmt.Sprintf("%-6s %-30s %-8s %8.2fms %6.2fms %6.2f%%\n",
				r.Type, r.Target, r.Status, r.Latency, r.Jitter, r.PacketLoss)
		}
		return modulepb.OKResponse(out), nil

	case "waddleperf status":
		testOK, managerOK := m.client.HealthCheck(ctx)
		out := fmt.Sprintf("Test Server:    %s\nManager Server: %s\n",
			boolStatus(testOK), boolStatus(managerOK))
		return modulepb.OKResponse(out), nil

	default:
		return modulepb.UnknownCommandResponse(req.CommandPath), nil
	}
}

func (m *Module) GetIcon(ctx context.Context) (*modulepb.IconResponse, error) {
	return &modulepb.IconResponse{}, nil
}

func boolStatus(ok bool) string {
	if ok {
		return "reachable"
	}
	return "unreachable"
}
