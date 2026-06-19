package reconcile

import (
	"context"
	"testing"

	"github.com/luddite-dev/deploy/internal/control"
)

type fakeRunner struct {
	applyCalls  []string
	removeCalls []string
}

func (f *fakeRunner) Apply(_ context.Context, name, composePath string) error {
	f.applyCalls = append(f.applyCalls, name+":"+composePath)
	return nil
}

func (f *fakeRunner) Remove(_ context.Context, name, composePath string) error {
	f.removeCalls = append(f.removeCalls, name+":"+composePath)
	return nil
}

func TestServiceApplySkipsSameVersionAndHandlesDelete(t *testing.T) {
	runner := &fakeRunner{}
	svc := New(t.TempDir(), runner)

	deploy := control.DesiredDeployment{
		NodeID:  "node-a",
		Version: 2,
		Spec: control.DeploymentSpec{
			Name:        "web",
			ComposeYAML: "services:\n  web:\n    image: nginx:latest\n",
		},
	}

	if _, err := svc.Apply(context.Background(), deploy); err != nil {
		t.Fatal(err)
	}
	if _, err := svc.Apply(context.Background(), deploy); err != nil {
		t.Fatal(err)
	}

	deleted := deploy
	deleted.Version = 3
	deleted.Deleted = true
	obs, err := svc.Apply(context.Background(), deleted)
	if err != nil {
		t.Fatal(err)
	}

	if len(runner.applyCalls) != 1 {
		t.Fatalf("apply calls = %d, want 1", len(runner.applyCalls))
	}
	if len(runner.removeCalls) != 1 {
		t.Fatalf("remove calls = %d, want 1", len(runner.removeCalls))
	}
	if obs.State != control.ApplySucceeded || obs.AppliedVersion != 3 {
		t.Fatalf("delete observed = %+v, want succeeded version 3", obs)
	}
}
