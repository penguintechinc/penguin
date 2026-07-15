// Package version exposes the build-time version of the penguin binaries.
package version

// Version is injected at build time via
// -ldflags "-X github.com/penguintechinc/penguin/internal/version.Version=$(cat .version)".
var Version = "dev"
