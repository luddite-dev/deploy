package state

import (
	"os"
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
	if desired.Version != 1 {
		t.Fatalf("first desired version = %d, want 1", desired.Version)
	}

	desired, err = store.PutDesiredDeployment("node-a", control.DeploymentSpec{
		Name:        "web",
		ComposeYAML: "services:\n  web:\n    image: nginx:1.27\n",
	})
	if err != nil {
		t.Fatal(err)
	}
	if desired.Version != 2 {
		t.Fatalf("second desired version = %d, want 2", desired.Version)
	}

	if err := store.PutObservedDeployment(control.ObservedDeployment{
		NodeID:         "node-a",
		Name:           "web",
		AppliedVersion: desired.Version,
		State:          control.ApplySucceeded,
	}); err != nil {
		t.Fatal(err)
	}

	desired, err = store.DeleteDesiredDeployment("node-a", "web")
	if err != nil {
		t.Fatal(err)
	}
	if desired.Version != 3 {
		t.Fatalf("deleted desired version = %d, want 3", desired.Version)
	}
	if !desired.Deleted {
		t.Fatal("deleted desired deployment = false, want true")
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
	if view.Desired.Version != 3 {
		t.Fatalf("desired version = %d, want 3", view.Desired.Version)
	}
	if !view.Desired.Deleted {
		t.Fatal("desired deployment not marked deleted after reopen")
	}
	if view.Observed == nil || view.Observed.AppliedVersion != 2 {
		t.Fatalf("observed version = %+v, want 2", view.Observed)
	}
}

func TestStoreDoesNotAdvanceMemoryWhenFlushFails(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")

	store, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}

	first, err := store.PutDesiredDeployment("node-a", control.DeploymentSpec{
		Name:        "web",
		ComposeYAML: "services:\n  web:\n    image: nginx:latest\n",
	})
	if err != nil {
		t.Fatal(err)
	}
	if first.Version != 1 {
		t.Fatalf("first desired version = %d, want 1", first.Version)
	}

	blockedDir := filepath.Join(t.TempDir(), "blocked")
	if err := os.Mkdir(blockedDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(blockedDir, "state.json"), []byte("not a directory"), 0o644); err != nil {
		t.Fatal(err)
	}
	store.path = filepath.Join(blockedDir, "state.json", "nested.json")

	if _, err := store.PutDesiredDeployment("node-a", control.DeploymentSpec{
		Name:        "web",
		ComposeYAML: "services:\n  web:\n    image: nginx:1.27\n",
	}); err == nil {
		t.Fatal("PutDesiredDeployment error = nil, want flush failure")
	}

	view, ok := store.GetDeploymentStatus("node-a", "web")
	if !ok {
		t.Fatal("deployment status missing after failed flush")
	}
	if view.Desired.Version != 1 {
		t.Fatalf("desired version after failed flush = %d, want 1", view.Desired.Version)
	}
	if view.Desired.Spec.ComposeYAML != "services:\n  web:\n    image: nginx:latest\n" {
		t.Fatalf("compose after failed flush = %q", view.Desired.Spec.ComposeYAML)
	}

	reopened, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	view, ok = reopened.GetDeploymentStatus("node-a", "web")
	if !ok {
		t.Fatal("persisted deployment status missing after failed flush")
	}
	if view.Desired.Version != 1 {
		t.Fatalf("persisted desired version after failed flush = %d, want 1", view.Desired.Version)
	}
}
