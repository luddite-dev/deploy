package reconcile

import (
	"context"
	"os"
	"path/filepath"
	"sync"

	"github.com/luddite-dev/deploy/internal/agent/runtime"
	"github.com/luddite-dev/deploy/internal/control"
)

type Service struct {
	root         string
	runner       runtime.Runner
	mu           sync.Mutex
	appliedByApp map[string]int
}

func New(root string, runner runtime.Runner) *Service {
	return &Service{
		root:         root,
		runner:       runner,
		appliedByApp: map[string]int{},
	}
}

func (s *Service) Apply(ctx context.Context, desired control.DesiredDeployment) (control.ObservedDeployment, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	composeDir := filepath.Join(s.root, desired.Spec.Name)
	composePath := filepath.Join(composeDir, "compose.yaml")
	if err := os.MkdirAll(composeDir, 0o755); err != nil {
		return control.ObservedDeployment{}, err
	}
	if err := os.WriteFile(composePath, []byte(desired.Spec.ComposeYAML), 0o644); err != nil {
		return control.ObservedDeployment{}, err
	}

	if desired.Deleted {
		if s.appliedByApp[desired.Spec.Name] != 0 {
			if err := s.runner.Remove(ctx, desired.Spec.Name, composePath); err != nil {
				return control.ObservedDeployment{}, err
			}
			delete(s.appliedByApp, desired.Spec.Name)
		}
		return control.ObservedDeployment{
			NodeID:         desired.NodeID,
			Name:           desired.Spec.Name,
			AppliedVersion: desired.Version,
			State:          control.ApplySucceeded,
		}, nil
	}

	if s.appliedByApp[desired.Spec.Name] != desired.Version {
		if err := s.runner.Apply(ctx, desired.Spec.Name, composePath); err != nil {
			return control.ObservedDeployment{
				NodeID:         desired.NodeID,
				Name:           desired.Spec.Name,
				AppliedVersion: desired.Version,
				State:          control.ApplyFailed,
				Message:        err.Error(),
			}, nil
		}
		s.appliedByApp[desired.Spec.Name] = desired.Version
	}

	return control.ObservedDeployment{
		NodeID:         desired.NodeID,
		Name:           desired.Spec.Name,
		AppliedVersion: desired.Version,
		State:          control.ApplySucceeded,
	}, nil
}
