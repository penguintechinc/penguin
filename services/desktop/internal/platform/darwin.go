//go:build darwin

package platform

func InterfaceName() string {
	return "utun1"
}

func ServiceName() string {
	return "io.penguintech.penguin"
}
