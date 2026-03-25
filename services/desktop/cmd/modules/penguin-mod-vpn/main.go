// penguin-mod-vpn is a go-plugin binary for the VPN module.
package main

import (
	"github.com/penguintechinc/penguin/services/desktop/internal/modules/vpn"
	pluginpkg "github.com/penguintechinc/penguin/services/desktop/pkg/plugin"
)

func main() {
	pluginpkg.Serve(&pluginpkg.ModuleAdapter{Mod: vpn.New()})
}
