// penguin-mod-skauswatch is a go-plugin binary for the SkaUsWatch module.
package main

import (
	"github.com/penguintechinc/penguin/services/desktop/internal/modules/skauswatch"
	pluginpkg "github.com/penguintechinc/penguin/services/desktop/pkg/plugin"
)

func main() {
	pluginpkg.Serve(&pluginpkg.ModuleAdapter{Mod: skauswatch.New()})
}
