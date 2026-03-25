// penguin-mod-killkrill is a go-plugin binary for the KillKrill module.
package main

import (
	"github.com/penguintechinc/penguin/services/desktop/internal/modules/killkrill"
	pluginpkg "github.com/penguintechinc/penguin/services/desktop/pkg/plugin"
)

func main() {
	pluginpkg.Serve(&pluginpkg.ModuleAdapter{Mod: killkrill.New()})
}
