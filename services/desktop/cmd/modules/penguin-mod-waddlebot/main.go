// penguin-mod-waddlebot is a go-plugin binary for the WaddleBot bridge module.
package main

import (
	"github.com/penguintechinc/penguin/services/desktop/internal/modules/waddlebot"
	pluginpkg "github.com/penguintechinc/penguin/services/desktop/pkg/plugin"
)

func main() {
	pluginpkg.Serve(&pluginpkg.ModuleAdapter{Mod: waddlebot.New()})
}
