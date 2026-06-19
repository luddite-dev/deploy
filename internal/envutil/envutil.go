package envutil

import "os"

// EnvOrDefault returns the named environment variable's value, or fallback if
// the variable is unset or empty.
func EnvOrDefault(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}
