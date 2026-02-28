package waddlebot

import "time"

// BridgeConfig holds connection settings for the WaddleBot bridge.
type BridgeConfig struct {
	APIURL      string
	CommunityID string
	UserID      string
	OBSHost     string
	OBSPort     int
	OBSPassword string
}

// BridgeStatus reports the current state of the bridge and OBS connections.
type BridgeStatus struct {
	Connected    bool
	CommunityID  string
	UserID       string
	BridgeID     string
	LastPoll     time.Time
	OBSConnected bool
	OBSScene     string
	Streaming    bool
	Recording    bool
}

// ActionRequest represents an action dispatched by the WaddleBot server.
type ActionRequest struct {
	ID          string            `json:"id"`
	Type        string            `json:"type"`
	Module      string            `json:"module"`
	Action      string            `json:"action"`
	Parameters  map[string]string `json:"parameters"`
	UserID      string            `json:"user_id"`
	CommunityID string            `json:"community_id"`
	Priority    int               `json:"priority"`
	Timeout     int               `json:"timeout"`
	CreatedAt   time.Time         `json:"created_at"`
	ExpiresAt   time.Time         `json:"expires_at"`
}

// ActionResponse reports the result of executing an ActionRequest.
type ActionResponse struct {
	ID       string                 `json:"id"`
	Success  bool                   `json:"success"`
	Result   map[string]interface{} `json:"result"`
	Error    string                 `json:"error,omitempty"`
	Duration int64                  `json:"duration"`
}

// PollResponse is returned by the bridge poll endpoint.
type PollResponse struct {
	Actions  []ActionRequest `json:"actions"`
	NextPoll time.Time       `json:"next_poll"`
	HasMore  bool            `json:"has_more"`
}

// RegistrationRequest is sent to the bridge register endpoint.
type RegistrationRequest struct {
	CommunityID  string       `json:"community_id"`
	UserID       string       `json:"user_id"`
	Version      string       `json:"version"`
	Platform     string       `json:"platform"`
	Capabilities []string     `json:"capabilities"`
	Modules      []ModuleInfo `json:"modules"`
}

// RegistrationResponse is returned by the bridge register endpoint.
type RegistrationResponse struct {
	BridgeID     string `json:"bridge_id"`
	PollInterval int    `json:"poll_interval"`
}

// ModuleInfo describes a bridge module and its supported actions.
type ModuleInfo struct {
	Name    string       `json:"name"`
	Version string       `json:"version"`
	Actions []ActionInfo `json:"actions"`
}

// ActionInfo describes a single action supported by a bridge module.
type ActionInfo struct {
	Name        string `json:"name"`
	Description string `json:"description"`
}

// RecentAction records a recently executed action for display in the GUI.
type RecentAction struct {
	Name      string
	Status    string
	Timestamp time.Time
}

// OBSConnectionInfo holds a snapshot of the OBS connection state.
type OBSConnectionInfo struct {
	State        string
	OBSVersion   string
	CurrentScene string
	Streaming    bool
	Recording    bool
}
