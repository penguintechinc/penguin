// Package plugin provides the go-plugin integration for the Penguin module system.
// It implements the HashiCorp go-plugin pattern where each module runs as a separate
// binary and communicates with the host via gRPC over stdin/stdout.
package plugin

import (
	"github.com/hashicorp/go-plugin"
)

const (
	// PluginName is the key used in the plugin map to identify module plugins.
	PluginName = "penguin-module-v1"
)

// Handshake is the shared handshake config that host and plugin must agree on.
// Changing these values breaks compatibility between host and plugin versions.
var Handshake = plugin.HandshakeConfig{
	ProtocolVersion:  1,
	MagicCookieKey:   "PENGUIN_MODULE_PLUGIN",
	MagicCookieValue: "penguin-module-v1",
}
