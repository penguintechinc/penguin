package nest

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/penguintechinc/penguin/services/desktop/pkg/desktop"
	"github.com/sirupsen/logrus"
)

// Client is a REST client for the Nest API.
type Client struct {
	api       *desktop.JSONClient
	authToken string
	logger    *logrus.Logger
}

// NewClient creates a Nest API client.
func NewClient(baseURL, authToken string, logger *logrus.Logger) *Client {
	baseURL = strings.TrimRight(baseURL, "/")
	c := &Client{
		api:       desktop.NewJSONClient(baseURL, 30*time.Second),
		authToken: authToken,
		logger:    logger,
	}
	if authToken != "" {
		c.api.GetToken = func() string {
			return authToken
		}
	}
	return c
}

// ListParams for filtering resource lists.
type ListParams struct {
	Page     int
	PageSize int
	TeamID   uint
	Status   string
}

// ListResources returns resources with optional filtering.
func (c *Client) ListResources(ctx context.Context, params *ListParams) ([]Resource, error) {
	path := "/api/v1/resources"
	if params != nil {
		q := []string{}
		if params.Page > 0 {
			q = append(q, fmt.Sprintf("page=%d", params.Page))
		}
		if params.PageSize > 0 {
			q = append(q, fmt.Sprintf("page_size=%d", params.PageSize))
		}
		if params.TeamID > 0 {
			q = append(q, fmt.Sprintf("team_id=%d", params.TeamID))
		}
		if params.Status != "" {
			q = append(q, fmt.Sprintf("status=%s", params.Status))
		}
		if len(q) > 0 {
			path += "?" + strings.Join(q, "&")
		}
	}

	var resources []Resource
	if err := c.api.DoJSON(ctx, "GET", path, nil, &resources); err != nil {
		return nil, err
	}
	return resources, nil
}

// GetResource returns a single resource.
func (c *Client) GetResource(ctx context.Context, id string) (*Resource, error) {
	var resource Resource
	if err := c.api.DoJSON(ctx, "GET", "/api/v1/resources/"+id, nil, &resource); err != nil {
		return nil, err
	}
	return &resource, nil
}

// CreateResource creates a new resource.
func (c *Client) CreateResource(ctx context.Context, req *CreateResourceRequest) (*Resource, error) {
	var resource Resource
	if err := c.api.DoJSON(ctx, "POST", "/api/v1/resources", req, &resource); err != nil {
		return nil, err
	}
	return &resource, nil
}

// UpdateResource updates a resource.
func (c *Client) UpdateResource(ctx context.Context, id string, req *UpdateResourceRequest) (*Resource, error) {
	var resource Resource
	if err := c.api.DoJSON(ctx, "PUT", "/api/v1/resources/"+id, req, &resource); err != nil {
		return nil, err
	}
	return &resource, nil
}

// DeleteResource deletes a resource.
func (c *Client) DeleteResource(ctx context.Context, id string) error {
	return c.api.DoJSON(ctx, "DELETE", "/api/v1/resources/"+id, nil, nil)
}

// GetResourceStats returns resource statistics.
func (c *Client) GetResourceStats(ctx context.Context, id string) (*ResourceStats, error) {
	var stats ResourceStats
	if err := c.api.DoJSON(ctx, "GET", "/api/v1/resources/"+id+"/stats", nil, &stats); err != nil {
		return nil, err
	}
	return &stats, nil
}

// GetConnectionInfo returns resource connection info.
func (c *Client) GetConnectionInfo(ctx context.Context, id string) (*ConnectionInfo, error) {
	var info ConnectionInfo
	if err := c.api.DoJSON(ctx, "GET", "/api/v1/resources/"+id+"/connection-info", nil, &info); err != nil {
		return nil, err
	}
	return &info, nil
}

// ListTeams returns all accessible teams.
func (c *Client) ListTeams(ctx context.Context) ([]Team, error) {
	var teams []Team
	if err := c.api.DoJSON(ctx, "GET", "/api/v1/teams", nil, &teams); err != nil {
		return nil, err
	}
	return teams, nil
}

// GetTeam returns a single team.
func (c *Client) GetTeam(ctx context.Context, id string) (*Team, error) {
	var team Team
	if err := c.api.DoJSON(ctx, "GET", "/api/v1/teams/"+id, nil, &team); err != nil {
		return nil, err
	}
	return &team, nil
}

// CreateTeam creates a new team.
func (c *Client) CreateTeam(ctx context.Context, req map[string]interface{}) (*Team, error) {
	var team Team
	if err := c.api.DoJSON(ctx, "POST", "/api/v1/teams", req, &team); err != nil {
		return nil, err
	}
	return &team, nil
}

// UpdateTeam updates a team.
func (c *Client) UpdateTeam(ctx context.Context, id string, req map[string]interface{}) (*Team, error) {
	var team Team
	if err := c.api.DoJSON(ctx, "PUT", "/api/v1/teams/"+id, req, &team); err != nil {
		return nil, err
	}
	return &team, nil
}

// DeleteTeam deletes a team.
func (c *Client) DeleteTeam(ctx context.Context, id string) error {
	return c.api.DoJSON(ctx, "DELETE", "/api/v1/teams/"+id, nil, nil)
}

// ListTeamMembers returns team members.
func (c *Client) ListTeamMembers(ctx context.Context, teamID string) (interface{}, error) {
	var members interface{}
	if err := c.api.DoJSON(ctx, "GET", "/api/v1/teams/"+teamID+"/members", nil, &members); err != nil {
		return nil, err
	}
	return members, nil
}

// AddTeamMember adds a member to a team.
func (c *Client) AddTeamMember(ctx context.Context, teamID string, req map[string]interface{}) (interface{}, error) {
	var result interface{}
	if err := c.api.DoJSON(ctx, "POST", "/api/v1/teams/"+teamID+"/members", req, &result); err != nil {
		return nil, err
	}
	return result, nil
}

// RemoveTeamMember removes a member from a team.
func (c *Client) RemoveTeamMember(ctx context.Context, teamID, memberID string) error {
	return c.api.DoJSON(ctx, "DELETE", "/api/v1/teams/"+teamID+"/members/"+memberID, nil, nil)
}
