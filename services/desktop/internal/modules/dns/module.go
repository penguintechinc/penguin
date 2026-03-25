package dns

import (
	"context"
	"fmt"
	"strings"

	"github.com/penguintechinc/penguin/services/desktop/internal/module"
	"github.com/penguintechinc/penguin/services/desktop/pkg/clischema"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
	"github.com/penguintechinc/penguin/services/desktop/pkg/uischema"
	"github.com/sirupsen/logrus"
)

// Module implements the DNS module for the desktop client.
type Module struct {
	module.BaseModule
	dohClient *DoHClient
	forwarder *Forwarder
}

func New() *Module    { return &Module{} }
func (m *Module) Name() string        { return "dns" }
func (m *Module) DisplayName() string { return "DNS Client" }
func (m *Module) Description() string { return "DNS-over-HTTPS client with local forwarding" }
func (m *Module) Version() string     { return "0.1.0" }

func (m *Module) Init(ctx context.Context, deps module.Dependencies) error {
	m.Logger = deps.Logger.(*logrus.Logger)
	m.Logger.WithField("module", m.Name()).Debug("Initializing DNS module")

	cfg := &DoHConfig{
		ServerURLs: []string{
			"https://dns.google/dns-query",
			"https://cloudflare-dns.com/dns-query",
		},
		AuthToken: deps.AuthToken,
		Timeout:   0,
	}
	m.dohClient = NewDoHClient(cfg, m.Logger)

	fwdCfg := &ForwarderConfig{
		ListenAddr: "127.0.0.1:53",
		ListenUDP:  true,
		ListenTCP:  false,
	}
	m.forwarder = NewForwarder(m.dohClient, fwdCfg, m.Logger)
	return nil
}

func (m *Module) Start(ctx context.Context) error {
	m.MarkStarted()
	m.Logger.WithField("module", m.Name()).Info("DNS module started")
	return nil
}

func (m *Module) Stop(ctx context.Context) error {
	if m.forwarder != nil {
		m.forwarder.Stop()
	}
	m.MarkStopped()
	m.Logger.WithField("module", m.Name()).Info("DNS module stopped")
	return nil
}

func (m *Module) HealthCheck(ctx context.Context) module.HealthStatus {
	if !m.IsStarted() {
		return m.NotStartedStatus()
	}
	if m.dohClient != nil {
		return module.HealthStatus{
			State:   module.HealthHealthy,
			Message: "DNS client ready",
			Details: map[string]string{
				"client":    "doh",
				"forwarder": fmt.Sprintf("%v", m.forwarder != nil && m.forwarder.IsRunning()),
			},
		}
	}
	return module.HealthStatus{State: module.HealthDegraded, Message: "DNS client not configured"}
}

// --- PluginModule interface ---

func (m *Module) GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error) {
	fwdStatus := "Status: Not Available"
	fwdBtnText := "Start Forwarder"
	if m.forwarder != nil {
		if m.forwarder.IsRunning() {
			fwdStatus = fmt.Sprintf("Status: Running on %s", m.forwarder.GetListenAddr())
			fwdBtnText = "Stop Forwarder"
		} else {
			fwdStatus = "Status: Stopped"
		}
	}

	return uischema.Panel(
		uischema.Scroll(
			uischema.VBox(
				uischema.Card("dns-query-card", "DNS Query Tool", "Interactive DNS-over-HTTPS lookup",
					uischema.HBox(
						uischema.Entry("dns-domain", "example.com"),
						uischema.Select("dns-type", []string{"A", "AAAA", "CNAME", "MX", "TXT", "NS", "SOA", "SRV", "PTR"}, "A"),
						uischema.Button("dns-query-btn", "Query"),
					),
					uischema.Card("dns-results-card", "Results", "",
						uischema.Label("dns-results", "Enter a domain and click Query"),
					),
				),
				uischema.Card("dns-fwd-card", "DNS Forwarder", "Local DNS forwarding to DoH",
					uischema.Label("dns-fwd-status", fwdStatus),
					uischema.HBox(
						uischema.Button("dns-fwd-toggle", fwdBtnText),
						uischema.Label("dns-fwd-note", "(Forwards traditional DNS queries)"),
					),
				),
				uischema.Card("dns-info-card", "About", "",
					uischema.RichText("dns-info", "**DNS Module v0.1.0**\n\n"+
						"This module provides:\n"+
						"- **DNS-over-HTTPS (DoH)** queries for privacy\n"+
						"- **Automatic failover** between multiple DoH servers\n"+
						"- **Local DNS forwarder** for transparent forwarding"),
				),
			),
		),
	), nil
}

func (m *Module) HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error) {
	switch event.WidgetID {
	case "dns-query-btn":
		// Query is handled by the domain and type fields' current values
		// In a real plugin scenario, the host tracks widget state
	case "dns-fwd-toggle":
		if m.forwarder != nil {
			if m.forwarder.IsRunning() {
				m.forwarder.Stop()
			} else {
				m.forwarder.Start(ctx)
			}
		}
	}
	return m.GetGUIPanel(ctx)
}

func (m *Module) GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error) {
	dnsCmd := clischema.Command("dns", "DNS client and forwarder management")
	clischema.WithSubcommands(dnsCmd,
		*clischema.CommandWithLong("query [domain] [type]", "Query DNS record via DoH",
			"Query a DNS record using DNS-over-HTTPS. Type defaults to A if not specified."),
		*clischema.CommandWithLong("forward [start|stop|status]", "Manage DNS forwarder",
			"Start, stop, or check status of the local DNS forwarder"),
		*clischema.Command("health", "Check DNS module health"),
	)
	return clischema.CommandList(*dnsCmd), nil
}

func (m *Module) ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	switch req.CommandPath {
	case "dns query":
		if m.dohClient == nil {
			return modulepb.ErrorResponse(fmt.Errorf("DNS client not initialized")), nil
		}
		if len(req.Args) == 0 {
			return modulepb.ErrorResponse(fmt.Errorf("domain argument required")), nil
		}
		domain := req.Args[0]
		recordType := "A"
		if len(req.Args) > 1 {
			recordType = req.Args[1]
		}
		resp, err := m.dohClient.Query(ctx, domain, recordType)
		if err != nil {
			return modulepb.ErrorResponse(err), nil
		}
		if len(resp.Answer) == 0 {
			return modulepb.OKResponse(fmt.Sprintf("No records found for %s (%s)\n", domain, recordType)), nil
		}
		var sb strings.Builder
		sb.WriteString(fmt.Sprintf("%-40s TTL    CLASS   TYPE    DATA\n", "NAME"))
		sb.WriteString(strings.Repeat("-", 80) + "\n")
		for _, answer := range resp.Answer {
			sb.WriteString(fmt.Sprintf("%-40s %-7d IN      %-7s %s\n",
				answer.Name, answer.TTL, RecordTypeName(answer.Type), answer.Data))
		}
		return modulepb.OKResponse(sb.String()), nil

	case "dns forward":
		action := "status"
		if len(req.Args) > 0 {
			action = req.Args[0]
		}
		switch action {
		case "start":
			if m.forwarder == nil {
				return modulepb.ErrorResponse(fmt.Errorf("forwarder not initialized")), nil
			}
			if m.forwarder.IsRunning() {
				return modulepb.OKResponse("Forwarder already running\n"), nil
			}
			if err := m.forwarder.Start(ctx); err != nil {
				return modulepb.ErrorResponse(err), nil
			}
			return modulepb.OKResponse("DNS forwarder started on 127.0.0.1:53\n"), nil
		case "stop":
			if m.forwarder == nil {
				return modulepb.ErrorResponse(fmt.Errorf("forwarder not initialized")), nil
			}
			m.forwarder.Stop()
			return modulepb.OKResponse("DNS forwarder stopped\n"), nil
		case "status":
			if m.forwarder == nil {
				return modulepb.OKResponse("Forwarder: Not initialized\n"), nil
			}
			if m.forwarder.IsRunning() {
				return modulepb.OKResponse(fmt.Sprintf("Forwarder: Running on %s\n", m.forwarder.GetListenAddr())), nil
			}
			return modulepb.OKResponse("Forwarder: Stopped\n"), nil
		default:
			return modulepb.ErrorResponse(fmt.Errorf("unknown action: %s (use start, stop, or status)", action)), nil
		}

	case "dns health":
		health := m.HealthCheck(ctx)
		out := fmt.Sprintf("Status: %s\nMessage: %s\n", health.State, health.Message)
		for k, v := range health.Details {
			out += fmt.Sprintf("%s: %s\n", k, v)
		}
		return modulepb.OKResponse(out), nil

	default:
		return modulepb.UnknownCommandResponse(req.CommandPath), nil
	}
}

func (m *Module) GetIcon(ctx context.Context) (*modulepb.IconResponse, error) {
	return &modulepb.IconResponse{}, nil
}
