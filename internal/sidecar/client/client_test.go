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
