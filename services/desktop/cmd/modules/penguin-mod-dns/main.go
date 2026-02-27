// penguin-mod-dns is a go-plugin binary for the DNS module.
package main

import (
	"github.com/penguintechinc/penguin/services/desktop/internal/modules/dns"
	pluginpkg "github.com/penguintechinc/penguin/services/desktop/pkg/plugin"
)

func main() {
	pluginpkg.Serve(&pluginpkg.ModuleAdapter{Mod: dns.New()})
}
