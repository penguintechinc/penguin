//go:build windows

package platform

func InterfaceName() string {
	return "penguin"
}

func ServiceName() string {
	return "PenguinTechClient"
}
