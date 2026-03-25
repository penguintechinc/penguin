// penguin-mod-waddleperf is a go-plugin binary for the WaddlePerf module.
package main

import (
	"github.com/penguintechinc/penguin/services/desktop/internal/modules/waddleperf"
	pluginpkg "github.com/penguintechinc/penguin/services/desktop/pkg/plugin"
)

func main() {
	pluginpkg.Serve(&pluginpkg.ModuleAdapter{Mod: waddleperf.New()})
}
