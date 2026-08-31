# FleetDM Coexistence

Guidance for running FleetDM (`fleetd`/`osqueryd`) alongside the penguin agent on the same endpoint.

## Recommendation: Deploy Both

Deploy FleetDM and the penguin agent together on managed endpoints. The two are **complementary, not competing**:

- **FleetDM** handles MDM enrollment, device inventory, osquery-based host monitoring, policy definitions, and compliance checks.
- **Penguin agent** handles product-specific modules (WaddleAI, Waddles, Tobogganing, Squawk, Skauswatch), agent self-protection, and OpenTelemetry/threat reporting.

Both solve different parts of the endpoint management problem.

## Responsibility Split

| Capability | Owner | Details |
|---|---|---|
| **MDM Enrollment** | FleetDM | Device identity, certificate provisioning, profile delivery |
| **Device Inventory** | FleetDM | Hardware, OS, installed software census |
| **Host Monitoring (osquery)** | FleetDM | System state snapshots, query-based health checks, alerts |
| **Policy & Compliance** | FleetDM | Compliance queries, device posture, remediation definitions |
| **Remediation** | FleetDM | Via osquery tables, MDM profiles, or scripts |
| **PenguinTech Product Modules** | Penguin Agent | WaddleAI, Waddles, Tobogganing, Squawk, Skauswatch |
| **Agent Self-Protection** | Penguin Agent | Tamper resistance for the agent's own binary and configuration |
| **OpenTelemetry Export** | Penguin Agent | Structured logs, metrics, traces; integration with observability backends |
| **Threat & Endpoint Reporting** | Penguin Agent | EDR signals, host telemetry, security-relevant events |

## Non-Interference (Critical)

**Penguin self-protection guards only the penguin agent's own processes — it never supervises, restarts, kills, or otherwise interferes with `fleetd` or `osqueryd`.**

The penguin watchdog (part of self-protection) relaunches only the `penguind` daemon and its own watchdog peer. It has no visibility into or control over FleetDM processes. The two agents operate completely independently.

See `docs/self-protection.md` for the full runbook; the key invariant is stated there: self-protection protects only the agent's own binary, config, and service registration.

## Detection & Observability

The penguin agent **detects** the presence and version of FleetDM binaries on startup and reports this information through two channels:

### OpenTelemetry Resource Attributes

When OTel export is enabled, resource attributes include:
- `fleet_dm.fleetd` — version of `fleetd` binary (omitted if not installed)
- `fleet_dm.osqueryd` — version of `osqueryd` binary (omitted if not installed)

All penguin telemetry exports carry these attributes, so SigNoz and other collectors see coexistence metadata alongside all traces and metrics.

### Agent Status (gRPC `GetStatus`)

The daemon status snapshot includes a `fleet_dm` object with:
- `fleetd_present` — boolean
- `fleetd_version` — version string, or empty string when the binary is absent
- `osqueryd_present` — boolean
- `osqueryd_version` — version string, or empty string when the binary is absent

This detection is **read-only** — the penguin agent reports what it finds but never controls FleetDM.

## Central Server Deployment (SP3, Forthcoming)

The PenguinTech central server Helm chart (SP3, forthcoming) will install the penguin agent, FleetDM Community Edition, and SigNoz together by default in a single namespace. This simplifies onboarding: operators deploy one chart and receive full endpoint management + monitoring + Penguin software capabilities out of the gate.

Until SP3 ships, deploy FleetDM separately using the [Fleet documentation](https://docs.fleetdm.com/) and the penguin agent via its dedicated chart.

## Upgrade & Rollback

Both agents use independent systemd units/launchd plists and do not share dependencies. Upgrade or troubleshoot each in isolation:

```bash
# Penguin agent (systemd)
sudo systemctl restart penguind
sudo journalctl -u penguind -n 50 -f

# FleetDM (systemd; unit name depends on install method — typically 'orbit' or 'fleetd')
sudo systemctl restart orbit
sudo journalctl -u orbit -n 50 -f
```

If one agent fails, the other continues running. Monitor both in your observability stack and alert on restarts or version drift.

## Troubleshooting

**FleetDM not detected by penguin agent:**
- Verify `fleetd` and `osqueryd` are installed and their binaries are in `$PATH`
- Check penguin agent logs: `journalctl -u penguind | grep fleet`
- Detection runs once at `penguind` startup; if FleetDM is installed AFTER `penguind` is already running, restart the agent with `sudo systemctl restart penguind` for it to be detected

**Overlapping monitoring coverage:**
- This is expected and beneficial: osquery provides low-level host state, penguin telemetry provides application/product-level signals
- Configure FleetDM queries and penguin module flags independently; there is no resource contention

**Both agents restart frequently:**
- Check system logs for resource exhaustion, permissions issues, or crashes in either daemon
- Self-protection restarts only `penguind` and its watchdog; frequent restarts suggest a config or environment issue, not interference from FleetDM
