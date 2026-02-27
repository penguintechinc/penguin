// penguin-mod-openziti is a go-plugin binary for the OpenZiti module.
package main

import (
	"github.com/penguintechinc/penguin/services/desktop/internal/modules/openziti"
	pluginpkg "github.com/penguintechinc/penguin/services/desktop/pkg/plugin"
)

func main() {
	pluginpkg.Serve(&pluginpkg.ModuleAdapter{Mod: openziti.New()})
}
