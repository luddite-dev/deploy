package state

import (
	"path/filepath"
	"testing"

	"github.com/luddite-dev/deploy/internal/control"
)

func TestStorePersistsNodesDesiredAndObservedState(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")

	store, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}

	if err := store.UpsertNode(control.NodeRecord{
		NodeID:       "node-a",
		EndpointAddr: `{"node_id":"peer-a"}`,
		Connected:    true,
		LastSeen:     "2026-06-19T00:00:00Z",
	}); err != nil {
		t.Fatal(err)
	}

	desired, err := store.PutDesiredDeployment("node-a", control.DeploymentSpec{
		Name:        "web",
		ComposeYAML: "services:\n  web:\n    image: nginx:latest\n",
	})
	if err != nil {
		t.Fatal(err)
	}

	if err := store.PutObservedDeployment(control.ObservedDeployment{
		NodeID:         "node-a",
		Name:           "web",
		AppliedVersion: desired.Version,
		State:          control.ApplySucceeded,
	}); err != nil {
		t.Fatal(err)
	}

	reopened, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}

	nodes := reopened.ListNodes()
	if len(nodes) != 1 {
		t.Fatalf("nodes = %d, want 1", len(nodes))
	}
	if nodes[0].EndpointAddr != `{"node_id":"peer-a"}` {
		t.Fatalf("endpoint addr = %q", nodes[0].EndpointAddr)
	}

	view, ok := reopened.GetDeploymentStatus("node-a", "web")
	if !ok {
		t.Fatal("deployment status missing after reopen")
	}
	if view.Desired.Version != 1 {
		t.Fatalf("desired version = %d, want 1", view.Desired.Version)
	}
	if view.Observed == nil || view.Observed.AppliedVersion != 1 {
		t.Fatalf("observed version = %+v, want 1", view.Observed)
	}
}
