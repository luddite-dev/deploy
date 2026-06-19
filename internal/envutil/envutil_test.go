package envutil

import (
	"os"
	"testing"
)

func TestEnvOrDefault_unsetOrEmpty_returns_fallback(t *testing.T) {
	t.Setenv("LUDDITE_TEST_ENVUTIL", "")
	if got := EnvOrDefault("LUDDITE_TEST_ENVUTIL", "fallback"); got != "fallback" {
		t.Fatalf("empty: got %q, want %q", got, "fallback")
	}
	os.Unsetenv("LUDDITE_TEST_ENVUTIL")
	if got := EnvOrDefault("LUDDITE_TEST_ENVUTIL", "fallback"); got != "fallback" {
		t.Fatalf("unset: got %q, want %q", got, "fallback")
	}
}

func TestEnvOrDefault_set_returns_value(t *testing.T) {
	t.Setenv("LUDDITE_TEST_ENVUTIL", "from-env")
	if got := EnvOrDefault("LUDDITE_TEST_ENVUTIL", "fallback"); got != "from-env" {
		t.Fatalf("got %q, want %q", got, "from-env")
	}
}
