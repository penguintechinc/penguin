// penguin-mod-ntp is a go-plugin binary for the NTP module.
package main

import (
	"github.com/penguintechinc/penguin/services/desktop/internal/modules/ntp"
	pluginpkg "github.com/penguintechinc/penguin/services/desktop/pkg/plugin"
)

func main() {
	pluginpkg.Serve(&pluginpkg.ModuleAdapter{Mod: ntp.New()})
}
