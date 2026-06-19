package runtime

import (
	"context"
	"os/exec"
)

type Runner interface {
	Apply(context.Context, string, string) error
	Remove(context.Context, string, string) error
}

type Podman struct{}

func (Podman) Apply(ctx context.Context, _ string, composePath string) error {
	cmd := exec.CommandContext(ctx, "podman", "compose", "-f", composePath, "up", "-d")
	return cmd.Run()
}

func (Podman) Remove(ctx context.Context, _ string, composePath string) error {
	cmd := exec.CommandContext(ctx, "podman", "compose", "-f", composePath, "down")
	return cmd.Run()
}
