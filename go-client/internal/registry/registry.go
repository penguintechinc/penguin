// Package registry maintains the list of built-in product modules available
// to the penguin daemon supervisor.
//
// To add a new product module, register its Factory in the Builtins slice:
//
//	var Builtins = []sdk.Factory{
//	    myproduct.Factory,
//	    anotherproduct.Factory,
//	}
//
// External (go-plugin) modules do not register here; they are discovered
// and loaded dynamically by the daemon at runtime.
package registry

import (
	"github.com/penguintechinc/penguin/internal/modules/squawk"
	"github.com/penguintechinc/penguin/internal/modules/tobogganing"
	"github.com/penguintechinc/penguin/pkg/sdk"
)

// Builtins is the registry of built-in compiled-in modules.
// Each module registers itself by adding its Factory to this slice.
var Builtins = []sdk.Factory{
	squawk.New,
	tobogganing.New,
}

// All returns all built-in modules.
func All() []sdk.Factory {
	return Builtins
}
