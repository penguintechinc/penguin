package daemon

import "github.com/penguintechinc/penguin/pkg/sdk"

// Module returns the loaded module by name, or false if not found.
// Safe for concurrent use.
func (s *Supervisor) Module(name string) (sdk.Module, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	ms, ok := s.loaded[name]
	if !ok {
		return nil, false
	}
	return ms.instance, true
}

// ModuleInfo returns identity metadata for any *registered* module, whether or
// not it is loaded. For unloaded modules it builds a throwaway instance from
// the factory, since Info() is defined to be callable before Init.
func (s *Supervisor) ModuleInfo(name string) (sdk.ModuleInfo, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	if ms, ok := s.loaded[name]; ok {
		return ms.instance.Info(), true
	}
	factory, ok := s.modules[name]
	if !ok {
		return sdk.ModuleInfo{}, false
	}
	return (*factory)().Info(), true
}

// Hosts returns the HostServices for a given module, or nil if not loaded.
// Safe for concurrent use.
func (s *Supervisor) Hosts(name string) sdk.HostServices {
	s.mu.RLock()
	defer s.mu.RUnlock()

	ms, ok := s.loaded[name]
	if !ok {
		return nil
	}
	return ms.host
}
