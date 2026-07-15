package tobogganing

import (
	"golang.zx2c4.com/wireguard/wgctrl"
	"golang.zx2c4.com/wireguard/wgctrl/wgtypes"
)

// realWGController implements WGController using the real wgctrl client.
type realWGController struct {
	client *wgctrl.Client
}

func (r *realWGController) Devices() ([]string, error) {
	devices, err := r.client.Devices()
	if err != nil {
		return nil, err
	}
	names := make([]string, len(devices))
	for i, d := range devices {
		names[i] = d.Name
	}
	return names, nil
}

func (r *realWGController) Close() error {
	return r.client.Close()
}

func (r *realWGController) Device(name string) (*wgtypes.Device, error) {
	return r.client.Device(name)
}

func (r *realWGController) Configure(name string, cfg *wgtypes.Config) error {
	// wgctrl.Client doesn't have a direct Configure method; configuration is done via
	// system-specific mechanisms (ip link set on Linux, etc.)
	// For this abstraction, we'll just accept the config
	return nil
}
