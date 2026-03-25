//go:build linux

package platform

// InterfaceName returns the default network interface name.
func InterfaceName() string {
	return "wg0"
}

// ServiceName returns the system service name.
func ServiceName() string {
	return "penguin-client"
}
