// penguin-mod-articdbm is a go-plugin binary for the ArticDBM module.
package main

import (
	"github.com/penguintechinc/penguin/services/desktop/internal/modules/articdbm"
	pluginpkg "github.com/penguintechinc/penguin/services/desktop/pkg/plugin"
)

func main() {
	pluginpkg.Serve(&pluginpkg.ModuleAdapter{Mod: articdbm.New()})
}
