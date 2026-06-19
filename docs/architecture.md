# Architecture

The first milestone uses three runtime pieces:

- a Go master service for operator HTTP requests, node records, and
  desired-state persistence
- a Go agent service for local Podman reconciliation on each node
- a Rust `iroh-bridge` sidecar on both the master and agent machines for Iroh
  connectivity

Nodes still connect outward from constrained networks, but the application logic
stays almost entirely in Go because the Iroh-specific work is isolated behind
the sidecar boundary.
