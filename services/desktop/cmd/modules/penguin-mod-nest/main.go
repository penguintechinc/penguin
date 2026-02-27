// penguin-mod-nest is a go-plugin binary for the Nest module.
package main

import (
	"github.com/penguintechinc/penguin/services/desktop/internal/modules/nest"
	pluginpkg "github.com/penguintechinc/penguin/services/desktop/pkg/plugin"
)

func main() {
	pluginpkg.Serve(&pluginpkg.ModuleAdapter{Mod: nest.New()})
}
