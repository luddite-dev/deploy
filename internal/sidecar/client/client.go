package client

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"

	"github.com/luddite-dev/deploy/internal/control"
)

type Client struct {
	baseURL string
	http    *http.Client
}

type identityResponse struct {
	EndpointAddrJSON string `json:"endpoint_addr_json"`
}

type desiredDispatch struct {
	EndpointAddrJSON string                    `json:"endpoint_addr_json"`
	Deployment       control.DesiredDeployment `json:"deployment"`
}

type observedDispatch struct {
	EndpointAddrJSON string                     `json:"endpoint_addr_json"`
	Deployment       control.ObservedDeployment `json:"deployment"`
}

func New(baseURL string) *Client {
	return &Client{baseURL: baseURL, http: http.DefaultClient}
}

func (c *Client) Identity(ctx context.Context) (string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.baseURL+"/v1/identity", nil)
	if err != nil {
		return "", err
	}
	res, err := c.http.Do(req)
	if err != nil {
		return "", err
	}
	defer res.Body.Close()
	var out identityResponse
	if err := json.NewDecoder(res.Body).Decode(&out); err != nil {
		return "", err
	}
	return out.EndpointAddrJSON, nil
}

func (c *Client) PublishDesired(ctx context.Context, endpointAddr string, dep control.DesiredDeployment) error {
	return c.post(ctx, "/v1/master/publish", desiredDispatch{EndpointAddrJSON: endpointAddr, Deployment: dep})
}

func (c *Client) ReportObserved(ctx context.Context, endpointAddr string, obs control.ObservedDeployment) error {
	return c.post(ctx, "/v1/agent/report", observedDispatch{EndpointAddrJSON: endpointAddr, Deployment: obs})
}

func (c *Client) PollDesired(ctx context.Context) ([]control.DesiredDeployment, error) {
	return getSlice[control.DesiredDeployment](ctx, c.http, c.baseURL+"/v1/agent/messages")
}

func (c *Client) PollObserved(ctx context.Context) ([]control.ObservedDeployment, error) {
	return getSlice[control.ObservedDeployment](ctx, c.http, c.baseURL+"/v1/master/reports")
}

func (c *Client) post(ctx context.Context, path string, body any) error {
	buf, err := json.Marshal(body)
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.baseURL+path, bytes.NewReader(buf))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	res, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	if res.StatusCode >= 300 {
		return fmt.Errorf("sidecar status %d", res.StatusCode)
	}
	return nil
}

func getSlice[T any](ctx context.Context, client *http.Client, url string) ([]T, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	res, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer res.Body.Close()
	var out []T
	if err := json.NewDecoder(res.Body).Decode(&out); err != nil {
		return nil, err
	}
	return out, nil
}
