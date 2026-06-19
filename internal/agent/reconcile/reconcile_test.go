package reconcile

import (
	"context"
	"errors"
	"strings"
	"testing"

	"github.com/luddite-dev/deploy/internal/control"
)

type fakeRunner struct {
	applyCalls  []string
	removeCalls []string
	applyErr    error
}

func (f *fakeRunner) Apply(_ context.Context, name, composePath string) error {
	f.applyCalls = append(f.applyCalls, name+":"+composePath)
	return f.applyErr
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

func TestServiceApplyReturnsFailedStateOnRunnerError(t *testing.T) {
	runner := &fakeRunner{applyErr: errors.New("boom")}
	svc := New(t.TempDir(), runner)

	deploy := control.DesiredDeployment{
		NodeID:  "node-a",
		Version: 2,
		Spec: control.DeploymentSpec{
			Name:        "web",
			ComposeYAML: "services:\n  web:\n    image: nginx:latest\n",
		},
	}

	obs, err := svc.Apply(context.Background(), deploy)
	if err != nil {
		t.Fatalf("err = %v, want nil", err)
	}
	if obs.State != control.ApplyFailed {
		t.Fatalf("state = %s, want %s", obs.State, control.ApplyFailed)
	}
	if !strings.Contains(obs.Message, "boom") {
		t.Fatalf("message = %q, want it to contain %q", obs.Message, "boom")
	}

	if _, err := svc.Apply(context.Background(), deploy); err != nil {
		t.Fatal(err)
	}
	if len(runner.applyCalls) != 2 {
		t.Fatalf("apply calls = %d, want 2 (same version re-invoked since appliedByApp not updated)", len(runner.applyCalls))
	}
}
