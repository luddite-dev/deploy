---
feature: help-and-running-docs
status: delivered
specs: []
plans:
  - docs/compose/plans/2026-06-19-help-and-running-docs.md
branch: help-and-running-docs
commits: c0669f5..597bea1
---

# Help Flags and Running Docs — Final Report

## What Was Built

All three control-plane binaries (`luddite-master`, `luddite-agent`, `iroh-bridge`) gained `--help` and proper CLI flag parsing. Every config value that previously came only from an env var is now a flag whose default comes from that env var (then a built-in default), so the same binary works both as a CLI tool and in a container/systemd unit without changes. Each binary also prints a one-line startup banner showing its resolved config so operators can see at a glance which address, sidecar, root, or endpoint it picked up.

A new `docs/running.md` walks a human through the prerequisites, building, the full config table for all three binaries, the four-process topology startup, exercising the master HTTP API with curl, and running the Go and Rust test suites. The README gained a short pointer to it under the Current Milestone section.

## Architecture

- `internal/envutil` — new shared helper `EnvOrDefault(key, fallback string) string`; used by both Go binaries to compute each flag's default from an env var. Treats unset and empty env values identically (both → fallback).
- `cmd/luddite-master/main.go` — switched from `os.Getenv` to stdlib `flag.String` with `envutil.EnvOrDefault` defaults for `--state` and `--sidecar`. Banner logs `state=... sidecar=... http=:8080 master-addr=<iroh endpoint>`. HTTP listen remains `:8080`.
- `cmd/luddite-agent/main.go` — flags `--sidecar`, `--root`, `--node-id`, `--master-api` (env defaults). Required-field validation at startup: empty `--root` or `--node-id` exits 1 with a clear message. Banner logs all resolved values plus the agent's Iroh endpoint.
- `rust/iroh-bridge/src/main.rs` — hand-rolled `parse_args` (handles `--addr <v>`, `--addr=<v>`, `-h`/`--help`, unknown-arg errors) and `resolve_addr(flag, env)` (flag wins over env wins over `127.0.0.1:7777`). Banner logs `iroh-bridge: sidecar http on <addr>`. No `clap` added — one flag, kept deps minimal.
- `docs/running.md` — single human-facing run/test guide derived from the actual server routes (`internal/master/httpapi/server.go`) and types (`internal/control/types.go`).
- `README.md` — one bullet pointing at `docs/running.md`.

### Design Decisions

- **Flags with env defaults, not flags-replace-env**: chosen because container and systemd wiring already references the existing env var names; keeping them lets operators move gradually to flags rather than break existing deployments. `flag.String(name, EnvOrDefault(env, default), "... (env VAR)")` puts the env-var name in `--help` automatically.
- **stdlib Go `flag`, hand-rolled Rust**: go.mod had zero third-party deps; adding a CLI library for two binaries' worth of flags would have been unjustified. Stdlib `flag` auto-dispatches `-h`/`--help` and exits 0. The Rust side has exactly one flag, so `clap` was overkill; six `#[test]`s cover the parser directly.
- **Startup banners included in this milestone**: the original success path logged nothing, so an operator could not tell what sidecar URL or Iroh endpoint a binary had actually resolved. Banners are one `log.Printf`/`eprintln!` line each — not testable as behavior, but verified manually.
- **Required-field validation for `--root`/`--node-id`**: the original code would have produced a malformed `POST /nodes/register` with empty `node_id` and surfaced as a confusing 4xx from the master. Failing fast at startup with the env-var hint is friendlier and is at a true input boundary.

## Usage

```bash
./luddite-master  --help        # prints -state, -sidecar with env + default
./luddite-agent   --help        # prints -sidecar, -root, -node-id, -master-api
./iroh-bridge     --help        # prints --addr and -h
```

Flag overrides env, env overrides built-in default. See `docs/running.md` for the full topology run, the API curl examples (register / list / create / status / delete), and the test commands.

## Verification

- `go build ./...` clean; `go test ./...` passes all packages (envutil + master/httpapi + master/state + agent/reconcile + sidecar/client).
- `cargo build` and `cargo test` clean for `rust/iroh-bridge`. Tests: 4 http_smoke + 1 iroh_loopback + 9 new unit tests in `src/main.rs` (4x `resolve_addr` combos, help short/long,_ADDR space-separated, `=` form, missing value, unknown arg).
- Manual: all three binaries exit 0 on `--help`; the agent exits 1 with the right message on empty `--root`; the bridge binds to `127.0.0.1:7011` under `--addr 127.0.0.1:7011` (banner appears + curl gets a router response, not connection refused).

## Journey Log

> Brief notes on what informed the final design. Not required reading.

- [lesson] Go stdlib `flag` exits 0 on `-h`/`-help`/`--help` automatically when ErrorHandling is ExitOnError (default) — no custom usage handler needed.
- [lesson] Iroh's `refresh_identity` takes ~3s before the sidecar is listening because of `endpoint.online().await` relay discovery, so a 1-second smoke-test wait fails; bumped the test loop to wait for the banner line.
- [lesson] The sidecar's local HTTP API has no `/health` route — curl against it returns 404, which is a valid "router responded" signal for a smoke test. Don't reference `/health` in human-facing docs.

## Source Materials

| File | Role | Notes |
|------|------|-------|
| `docs/compose/plans/2026-06-19-help-and-running-docs.md` | Implementation plan | Complete; design approved in conversation, no separate spec doc written |
