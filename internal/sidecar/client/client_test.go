package client

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/luddite-dev/deploy/internal/control"
)

func TestClientIdentityAndPublishDesired(t *testing.T) {
	var got map[string]any
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/v1/identity":
			w.Header().Set("Content-Type", "application/json")
			_ = json.NewEncoder(w).Encode(map[string]string{"endpoint_addr_json": `{"node_id":"local-sidecar"}`})
		case "/v1/master/publish":
			if err := json.NewDecoder(r.Body).Decode(&got); err != nil {
				t.Fatal(err)
			}
			w.WriteHeader(http.StatusAccepted)
		default:
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	defer srv.Close()

	cli := New(srv.URL)
	identity, err := cli.Identity(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if identity != `{"node_id":"local-sidecar"}` {
		t.Fatalf("identity = %q", identity)
	}

	err = cli.PublishDesired(context.Background(), `{"node_id":"agent-sidecar"}`, control.DesiredDeployment{
		NodeID:  "node-a",
		Version: 1,
		Spec: control.DeploymentSpec{
			Name:        "web",
			ComposeYAML: "services: {}\n",
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	if got["endpoint_addr_json"] != `{"node_id":"agent-sidecar"}` {
		t.Fatalf("publish body = %+v", got)
	}
}

func TestClientPollDesired(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/agent/messages" || r.Method != http.MethodGet {
			w.WriteHeader(http.StatusNotFound)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode([]control.DesiredDeployment{{
			NodeID:  "node-a",
			Version: 3,
			Spec: control.DeploymentSpec{
				Name:        "web",
				ComposeYAML: "services: {}\n",
			},
			Deleted: false,
		}, {
			NodeID:  "node-b",
			Version: 7,
			Spec: control.DeploymentSpec{
				Name:        "api",
				ComposeYAML: "services: {api: {image: x}}\n",
			},
			Deleted: true,
		}})
	}))
	defer srv.Close()

	cli := New(srv.URL)
	got, err := cli.PollDesired(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 2 {
		t.Fatalf("len = %d", len(got))
	}
	if got[0].NodeID != "node-a" || got[0].Version != 3 || got[0].Spec.Name != "web" || got[0].Spec.ComposeYAML != "services: {}\n" || got[0].Deleted {
		t.Fatalf("got[0] = %+v", got[0])
	}
	if got[1].NodeID != "node-b" || got[1].Version != 7 || got[1].Spec.Name != "api" || got[1].Deleted != true {
		t.Fatalf("got[1] = %+v", got[1])
	}
}

func TestClientPollObserved(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/master/reports" || r.Method != http.MethodGet {
			w.WriteHeader(http.StatusNotFound)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode([]control.ObservedDeployment{{
			NodeID:         "node-a",
			Name:           "web",
			AppliedVersion: 3,
			State:          control.ApplySucceeded,
			Message:        "",
		}, {
			NodeID:         "node-b",
			Name:           "api",
			AppliedVersion: 7,
			State:          control.ApplyFailed,
			Message:        "boom",
		}})
	}))
	defer srv.Close()

	cli := New(srv.URL)
	got, err := cli.PollObserved(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 2 {
		t.Fatalf("len = %d", len(got))
	}
	if got[0].NodeID != "node-a" || got[0].Name != "web" || got[0].AppliedVersion != 3 || got[0].State != control.ApplySucceeded || got[0].Message != "" {
		t.Fatalf("got[0] = %+v", got[0])
	}
	if got[1].NodeID != "node-b" || got[1].Name != "api" || got[1].AppliedVersion != 7 || got[1].State != control.ApplyFailed || got[1].Message != "boom" {
		t.Fatalf("got[1] = %+v", got[1])
	}
}

func TestClientReportObserved(t *testing.T) {
	var got observedDispatch
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/agent/report" || r.Method != http.MethodPost {
			w.WriteHeader(http.StatusNotFound)
			return
		}
		if err := json.NewDecoder(r.Body).Decode(&got); err != nil {
			t.Fatal(err)
		}
		w.WriteHeader(http.StatusAccepted)
	}))
	defer srv.Close()

	obs := control.ObservedDeployment{
		NodeID:         "node-a",
		Name:           "web",
		AppliedVersion: 3,
		State:          control.ApplySucceeded,
		Message:        "ok",
	}
	if err := New(srv.URL).ReportObserved(context.Background(), `{"node_id":"agent-sidecar"}`, obs); err != nil {
		t.Fatal(err)
	}
	if got.EndpointAddrJSON != `{"node_id":"agent-sidecar"}` {
		t.Fatalf("endpoint_addr_json = %q", got.EndpointAddrJSON)
	}
	if got.Deployment.NodeID != "node-a" || got.Deployment.Name != "web" || got.Deployment.AppliedVersion != 3 || got.Deployment.State != control.ApplySucceeded || got.Deployment.Message != "ok" {
		t.Fatalf("deployment = %+v", got.Deployment)
	}
}

func TestClientGetErrorStatus(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer srv.Close()

	cli := New(srv.URL)
	if _, err := cli.Identity(context.Background()); err == nil {
		t.Fatal("Identity: expected error for status 500")
	}
	if _, err := cli.PollDesired(context.Background()); err == nil {
		t.Fatal("PollDesired: expected error for status 500")
	}
	if _, err := cli.PollObserved(context.Background()); err == nil {
		t.Fatal("PollObserved: expected error for status 500")
	}
}
