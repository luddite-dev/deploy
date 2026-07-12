# Iroh Transport Swap — Implementation Plan

Design spec: [`docs/compose/specs/2026-07-12-iroh-transport-design.md`](../specs/2026-07-12-iroh-transport-design.md)

Branch: `iroh-transport` (7 commits ahead of `main`).

## Task 1 — Iroh transport module ✅

Create `lib/transport/src/iroh/` with three files:

- `mod.rs` — module declarations.
- `framing.rs` — `FramedWriter`/`FramedReader` with 4-byte BE length-prefix
  framing. 5 unit tests.
- `secret.rs` — `load_secret_key`/`save_secret_key` for 32-byte Iroh
  `SecretKey` persistence. 1 unit test.
- `endpoint.rs` — `create_core_endpoint` (listener with ALPN) and
  `create_periphery_endpoint` (dialer, no ALPN). ALPN = `b"luddite/control/1"`.

Commit: `f1b0e1dda` — "feat: add iroh transport module (framing, secret key, endpoint setup)"

## Task 2 — Type changes ✅

Rewrite `LoginMessage` (`client/periphery/rs/src/transport/login.rs`) from
9 variants to 3: `OnboardingToken(String)`, `EndpointId(String)`, `Success`.

Update config types:
- `CoreConfig`: `iroh_secret_key` (replaces `private_key`),
  `iroh_periphery_endpoint_ids` (replaces `periphery_public_keys`),
  `first_server_endpoint_id` (replaces `first_server_address`).
- `PeripheryConfig`: `iroh_secret_key` (replaces `private_key`),
  `core_endpoint_addrs` (replaces `core_public_keys`),
  `onboarding_key` (replaces `passkey`).
- `ServerConfig`: delete `address`, `insecure_tls`, `passkey`.
- `ServerInfo`: `endpoint_id` (replaces `public_key`).

Commit: `e8271e55e` — "refactor: rewrite LoginMessage for Iroh (OnboardingToken/EndpointId/Success)"

## Task 3 — Core connection rewrite ✅

`bin/core/src/connection/`:
- `server.rs` — `run_accept_loop` (Iroh accept), `handle_connection` (login
  dispatch), `handle_existing_connection` (DB lookup + allowlist),
  `handle_onboarding_connection` (token validation + `create_or_update_server`
  + `handle_socket`), `create_or_update_server` (handles duplicate-name),
  `create_server_maybe_builder`.
- `client.rs` — **deleted** (Core no longer dials out).
- `config.rs` — `core_secret_key()` loads Iroh `SecretKey`,
  `iroh_periphery_endpoint_ids()` reads allowlist.

Commit: `902c7ce72` — "refactor: swap WebSocket/Noise transport to Iroh (QUIC-based)"
(this commit also includes Tasks 4 + 5 — they form a single compilation unit)

## Task 4 — Periphery connection rewrite ✅

`bin/periphery/src/connection/`:
- `client.rs` — `handler` (Iroh dial + retry loop with `AtomicBool`
  onboarding tracking), `connect_to_core` (`endpoint.connect(addr, ALPN)`).
  Sends `OnboardingToken`+`EndpointId` or just `EndpointId`, awaits `Success`,
  enters `handle_socket`.
- `server.rs` — **deleted** (Periphery no longer listens).
- `mod.rs` — `handle_socket` (bidirectional forwarding via `tokio::select!`),
  `handle_request` (per-request task spawn).
- `state.rs` — `periphery_secret_key()` loads from
  `{root_directory}/keys/iroh.key`.
- `helpers.rs` — SSL cert functions deleted.

Commit: `902c7ce72` (combined with Task 3).

## Task 5 — Delete old transport modules ✅

- Delete `lib/transport/src/auth.rs` (Noise handshake).
- Delete `lib/transport/src/websocket/` (WS trait family).
- Delete `lib/transport/src/timeout.rs` (WS timeout wrapper).
- Rewrite `lib/transport/src/lib.rs` and `lib/transport/src/channel.rs` to
  remove WS/Noise references.
- Clean up `Cargo.toml` — remove `mogh-pki`, `tungstenite`, `axum-extra` ws
  feature, `rustls` dependencies used only by old transport.

Commit: `9509b6e55` — "chore: clean up unused imports and deps after Iroh transport swap"

## Task 6 — Update docs ✅

- `readme.md` — M3 section with transport description, deleted files, what
  survives.
- `roadmap.md` — M3 marked ✅.
- `docs/forking.md` — Drop Rule 4 for WS/Noise transport changes, field
  renaming patterns.

Commit: `bd04dd795` — "docs: update readme, roadmap, and forking docs for Iroh transport swap (M3)"

## Protocol bug fixes (from live testing) ✅

Two additional commits fixing 7 bugs found during live integration testing:

- `e19015670` — Bugs 1-3: onboarding deadlock, false Success, server-already-exists.
- `0aa5d6c73` — Bugs 4-7: missing handle_socket, ObjectId query, disabled-server
  false Success, Periphery onboarding tracking.

See design spec §"Protocol bugs found during live testing" for details.

## Verification ✅

- `cargo check --workspace` — 0 errors, 0 warnings.
- `cargo test -p transport --lib iroh` — 5/5 pass.
- `cargo fmt -- --check` — pass.
- Live integration on 2 real servers — onboarding, data channel, reconnection,
  stability all verified. See design spec [S9].
