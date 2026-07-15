# Iroh Transport Swap Design

Replaces Komodo's WebSocket + mutual Noise XX handshake transport layer with an
Iroh-native transport (QUIC + TLS 1.3 with RFC 7250 Raw Public Keys). This is a
full replacement — no adapter, no double-auth.

## [S1] Problem

Komodo's transport stack has three layers that don't compose cleanly with Iroh:

1. **Noise XX handshake** (`lib/transport/src/auth.rs`) — a 3-message
   `MutualNoiseHandshake` over `mogh_pki::mutual`
   (`Noise_XX_25519_ChaChaPoly_BLAKE2s`). Iroh already provides TLS 1.3 over
   QUIC with raw public key mutual auth. Running Noise on top of Iroh would
   double-encrypt every connection for no security gain.

2. **`Websocket` trait** (`lib/transport/src/websocket/mod.rs`) — a
   WS-frame-oriented abstraction. Iroh streams are raw byte-streams
   (`SendStream`/`RecvStream`), not message-framed. Adapting the WS trait to
   Iroh would require a length-prefix framing hack, and the trait adds no value
   once the only implementation is Iroh.

3. **Bidirectional connection model** — Core can dial Periphery or Periphery can
   dial Core, controlled by `ServerConfig.address`. Iroh's NAT traversal (UDP
   hole-punching + relay fallback) makes a single direction (Periphery→Core)
   sufficient for all topologies, including Core behind NAT.

## [S2] Solution Overview

Six design decisions, each with rationale:

### D1 — Onboarding = Bearer token over Iroh

Periphery connects to Core's `EndpointId` via Iroh (connection is mutually
authenticated by Iroh's TLS 1.3). Sends onboarding token as the first message on
the first bidi stream. Core validates against `OnboardingKey` DB records
(`onboarding_keys` collection), then registers Periphery's `EndpointId` on the
`Server` entity. Reuses `CreateOnboardingKey` API unchanged; only the wire
mechanism changes.

Chosen over "delegated Iroh SecretKey" (too complex — Core would need to
distribute per-Periphery secret keys) and "manual EndpointId registration" (too
manual — operator must capture and paste EndpointId strings).

### D2 — Replace transport entirely

Delete `lib/transport/src/auth.rs` (Noise handshake),
`lib/transport/src/websocket/` (WS trait family), and
`lib/transport/src/timeout.rs` (WS-specific timeout wrapper) entirely. Build a
new Iroh-native transport module from scratch.

The `TransportMessage` wire protocol (Request/Response/Terminal with UUID
multiplexing) survives, riding directly on Iroh QUIC bidi streams. Only
addition: length-prefix framing (4-byte BE length + bincode payload) for Iroh's
byte-stream read/write.

Chosen over an adapter (would leave dead WS/Noise code and a framing hack).

### D3 — Single persistent bidi stream + UUID multiplexing

One QUIC bidi stream per Core-Periphery connection (mirrors the existing WS
model). `TransportMessage` + `WithChannel<Uuid>` survives almost unchanged.

Chosen over one-stream-per-RPC (would require rewriting the entire channel
routing layer) and `iroh-irpc` (adds a dependency; existing `#[derive(Resolve)]`
dispatch already handles RPC routing).

### D4 — Single ALPN `luddite/control/1`

All connection traffic multiplexed on a single ALPN. No separate streaming
plane. A second ALPN can be added later if a distinct streaming service emerges.

### D5 — App-layer allowlist check

Accept all connections at the Iroh level. First message on any new bidi stream
must be a `LoginMessage` — either `OnboardingToken(String)` (new nodes) or
`EndpointId(String)` (known nodes reconnecting). Core validates against DB
(`onboarding_keys` collection) or allowlist (derived from registered `Server`
entities with non-empty `endpoint_id`).

Chosen over Iroh connection hooks (onboarding connections come from unknown
`EndpointId`s, so the hook would need to allow all anyway).

### D6 — Unify to Periphery→Core direction

Periphery always initiates connection to Core's `EndpointId`. Core is the
listener. Eliminates `ServerConfig.address` direction flag and the entire
bidirectional Core→Periphery / Periphery→Core model.

Iroh's NAT traversal (hole-punching + relay fallback) handles all topologies
including Core behind NAT. Chosen over keeping bidirectional model (relay
fallback already makes single-direction robust).

## [S3] What Survives Unchanged

- `TransportMessage` enum (Request/Response/Terminal variants)
- `WithChannel<Uuid>` multiplexing
- `PeripheryRequest` enum + `#[derive(Resolve)]` dispatch
- `PeripheryClient::request<T>`
- `ResponseChannels` / `TerminalChannels` routing
- `PeripheryConnections` registry (keyed by `server_id: String`)
- 300+ `server_id` references across `resource/`/`sync/`/`api/execute/` (these
  reference a resource id, not a network address)

## [S4] What Gets Deleted

| File                                         | Reason                                        |
| -------------------------------------------- | --------------------------------------------- |
| `lib/transport/src/auth.rs`                  | Noise XX handshake — replaced by Iroh TLS 1.3 |
| `lib/transport/src/websocket/`               | WS trait family — Iroh streams are raw bytes  |
| `lib/transport/src/timeout.rs`               | WS-specific timeout wrapper                   |
| `bin/core/src/connection/client.rs`          | Core no longer dials out                      |
| `bin/periphery/src/connection/server.rs`     | Periphery no longer listens                   |
| `bin/periphery/src/helpers.rs` SSL functions | No self-signed certs needed                   |

## [S5] What Gets Rewritten

### `LoginMessage` (`client/periphery/rs/src/transport/login.rs`)

Old variants (9): `Nonce`, `Handshake`, `OnboardingFlow`, `PublicKey`,
`V1PasskeyFlow`, `V1Passkey`, etc.

New variants (3):

```rust
pub enum LoginMessage {
    OnboardingToken(String),  // new nodes — validated against DB
    EndpointId(String),       // known nodes reconnecting — validated against allowlist
    Success,                  // Core accepts the connection
}
```

Wire format: variant byte (0=OnboardingToken, 1=EndpointId, 2=Success) + content
bytes, wrapped in `EncodedTransportMessage`.

### Core connection layer (`bin/core/src/connection/`)

- `server.rs` — Iroh accept loop (`run_accept_loop`), login dispatch
  (`handle_connection`), existing-server login (`handle_existing_connection`),
  onboarding login (`handle_onboarding_connection`), `handle_socket`
  (bidirectional message forwarding).
- `client.rs` — **deleted** (Core no longer dials out).

### Periphery connection layer (`bin/periphery/src/connection/`)

- `client.rs` — Iroh dialer (`handler`), `connect_to_core`, login flow (sends
  OnboardingToken+EndpointId or just EndpointId, awaits Success, enters
  `handle_socket`). Retry loop with `periphery_client::CONNECTION_RETRY_SECONDS`
  backoff.
- `server.rs` — **deleted** (Periphery no longer listens).
- `mod.rs` — `handle_socket` (bidirectional message forwarding via
  `tokio::select!`), `handle_request` (per-request task spawn).
- `state.rs` — `periphery_secret_key()` loads Iroh `SecretKey` from
  `{root_directory}/keys/iroh.key`.
- `helpers.rs` — SSL cert functions deleted.

## [S6] What's New — Iroh Transport Module

New module at `lib/transport/src/iroh/`:

### `framing.rs`

Length-prefix framing for Iroh byte-streams:

```rust
pub struct FramedWriter<W: AsyncWrite> { /* ... */ }
pub struct FramedReader<R: AsyncRead> { /* ... */ }
```

Each message: 4-byte big-endian length + bincode payload. Max message size: 16
MiB. 5 unit tests (round-trip, empty, multiple, EOF error, size limit).

### `secret.rs`

Iroh `SecretKey` persistence:

```rust
pub fn load_secret_key(path: &str) -> anyhow::Result<SecretKey>
pub fn save_secret_key(key: &SecretKey, path: &Path) -> anyhow::Result<()>
```

32 raw bytes on disk (not base64/PEM). Auto-generates + persists on first run so
`EndpointId` is stable across restarts. 1 unit test.

### `endpoint.rs`

Iroh `Endpoint` factory:

```rust
pub async fn create_core_endpoint(secret_key: SecretKey) -> anyhow::Result<Endpoint>
pub async fn create_periphery_endpoint(secret_key: SecretKey) -> anyhow::Result<Endpoint>
```

Core: binds with ALPNs `["luddite/control/1"]` (listener). Periphery: binds
without ALPNs (dialer only). Both use `presets::N0` (Iroh default relay + STUN).

## [S7] Connection Lifecycle

### First connection (onboarding)

1. Periphery starts, loads/generates Iroh `SecretKey`, logs `EndpointId`.
2. Periphery dials Core's `EndpointId` via Iroh (UDP hole-punching or relay).
3. Periphery opens a bidi stream (`open_bi`).
4. Periphery sends `LoginMessage::OnboardingToken(token)` followed by
   `LoginMessage::EndpointId(our_endpoint_id)`.
5. Core validates token against `onboarding_keys` DB collection.
6. Core calls `create_or_update_server` — checks if a `Server` with the
   onboarding key's public key as name already exists; creates if not, reuses if
   found.
7. Core inserts `PeripheryConnection`, sends `LoginMessage::Success`.
8. Core enters `handle_socket` (bidirectional message forwarding loop).
9. Periphery receives `Success`, enters `handle_socket`.

### Reconnection (known node)

1. Periphery dials Core's `EndpointId`.
2. Periphery opens a bidi stream.
3. Periphery sends `LoginMessage::EndpointId(our_endpoint_id)`.
4. Core looks up `Server` by `endpoint_id` in DB.
5. If found and enabled: insert `PeripheryConnection`, send `Success`, enter
   `handle_socket`.
6. If not found: return `Err` (connection drops, Periphery retries).

### Connection drop

When `handle_socket` returns (stream closed), both sides clean up. Periphery
retries with `CONNECTION_RETRY_SECONDS` backoff. Within a process lifetime,
Periphery tracks onboarding completion with an `AtomicBool` — after a successful
onboarding, it uses `EndpointId` login for subsequent reconnects. On process
restart, it falls back to `OnboardingToken` (stateless).

## [S8] Environment Variables

### Core

| Env var                              | Default                      | Purpose                                                                                                                           |
| ------------------------------------ | ---------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `KOMODO_IROH_SECRET_KEY`             | `file:/config/keys/iroh.key` | Iroh `SecretKey` (32 raw bytes). Auto-generates on first start.                                                                   |
| `KOMODO_IROH_SECRET_KEY_FILE`        | —                            | Alternative: path to key file.                                                                                                    |
| `KOMODO_IROH_PERIPHERY_ENDPOINT_IDS` | —                            | Allowlist of known Periphery `EndpointId`s (comma-separated). If absent, relies on DB-registered `Server` entities.               |
| `KOMODO_FIRST_SERVER_ENDPOINT_ID`    | —                            | Auto-creates enabled first `Server` entity with this `EndpointId`. Aliases: `KOMODO_FIRST_SERVER`, `KOMODO_FIRST_SERVER_ADDRESS`. |
| `KOMODO_FIRST_SERVER_NAME`           | —                            | Name for the first server entity.                                                                                                 |

### Periphery

| Env var                          | Default                               | Purpose                                                                            |
| -------------------------------- | ------------------------------------- | ---------------------------------------------------------------------------------- |
| `PERIPHERY_IROH_SECRET_KEY`      | `file:{root_directory}/keys/iroh.key` | Iroh `SecretKey` (32 raw bytes). Auto-generates.                                   |
| `PERIPHERY_IROH_SECRET_KEY_FILE` | —                                     | Alternative: path to key file.                                                     |
| `PERIPHERY_CORE_ENDPOINT_ADDRS`  | —                                     | Core's `EndpointId`(s). Comma-separated. Aliases: `PERIPHERY_CORE_ENDPOINT_ADDR`.  |
| `PERIPHERY_ONBOARDING_KEY`       | —                                     | Bearer token for first connection.                                                 |
| `PERIPHERY_CONNECT_AS`           | —                                     | **Hard requirement** when `core_endpoint_addrs` is set. Server name to connect as. |

### Deleted env vars

`KOMODO_PRIVATE_KEY`, `KOMODO_PERIPHERY_PUBLIC_KEYS`, `KOMODO_PASSKEY`,
`KOMODO_ADDRESS`, `KOMODO_INSECURE_TLS`, `PERIPHERY_CORE_PUBLIC_KEYS`,
`PERIPHERY_PASSKEY`, `PERIPHERY_PORT`, `PERIPHERY_BIND_IP`,
`PERIPHERY_ALLOWED_IPS`, `PERIPHERY_SSL_ENABLED`, `PERIPHERY_SSL_CERT_FILE`,
`PERIPHERY_SSL_KEY_FILE`.

## [S9] Verification

### Unit tests

- `lib/transport/src/iroh/framing.rs` — 5 tests: round-trip, empty message,
  multiple messages, EOF error, size limit.
- `lib/transport/src/iroh/secret.rs` — 1 test: key persistence (generate → save
  → reload → assert equal).

### Compile-time

- `cargo check --workspace` — 0 errors, 0 warnings.
- `cargo fmt -- --check` — pass.

### Live integration (2 servers)

Tested on real hardware:

- **Server 1** (`ac@luddite.dev`, RHEL 10): Core. EndpointId
  `9078a7072eb87ee66a4a89806ae2202ae2c8ea64f3d07977953a5ad4073dfa8a`.
- **Server 2** (`root@45.86.125.236`, RHEL 10): Periphery. EndpointId
  `b2414f6517d98a7d0b57fed5ca647ccabfc5839f4f5f006479765b753c0c2bd3`.
- **Server 3** (`root@23.254.215.230`, Ubuntu 20.04): excluded — Podman 3.4.2
  too old (requires Podman 4+ for `volume export`/`import`).

Verified:

- ✅ Core starts with stable `EndpointId` (key persisted to disk).
- ✅ Periphery connects to Core via Iroh (relay-assisted NAT traversal).
- ✅ Onboarding flow: token → create server → register `EndpointId` → Success.
- ✅ Data channel: `GetServerState` → `{"status": "Ok"}` (round-trip RPC).
- ✅ Reconnection: process restart → re-onboard → stable connection.
- ✅ Connection stability: no retry loops after 60+ seconds.

## [S10] Out of Scope

- **Iroh SecretKey rotation** — no mechanism to rotate keys after deployment.
  Future: API endpoint to generate a new key and update the `Server` entity.
- **Periphery↔Periphery connections** — not needed; all traffic flows through
  Core.
- **Separate streaming ALPN** — all traffic on `luddite/control/1`. A second
  ALPN can be added if a streaming service emerges.
- **Connection-level authorization beyond onboarding** — Iroh TLS 1.3
  authenticates the peer; app-layer allowlist checks `EndpointId` on first
  message. No per-RPC auth.

## Protocol bugs found during live testing

Seven protocol bugs were discovered and fixed during live integration testing:

1. **Onboarding login deadlock** — Core expected `OnboardingToken` then
   `EndpointId` as two consecutive messages; Periphery only sent
   `OnboardingToken` then waited for `Success`. Fix: Periphery sends both before
   waiting.

2. **False Success on not-found** — Core sent `Success` even when no server
   matched the `EndpointId`. Fix: removed `Success` from error paths.

3. **Server-already-exists on re-onboarding** — `CreateServer` failed on
   reconnect with the same onboarding key (duplicate name). Fix: added
   `create_or_update_server` that checks for existing server by name first.

4. **Missing `handle_socket` after onboarding** — `handle_onboarding_connection`
   sent `Success` and returned without entering the data exchange loop. Fix:
   added server fetch + `PeripheryConnection` insert + `handle_socket` call.

5. **ObjectId query bug** — Post-onboarding server lookup used string `_id`
   instead of `id_or_name_filter`. Fix: use `id_or_name_filter`.

6. **Disabled server false Success** — Same false-Success bug in disabled-server
   check. Fix: removed.

7. **Periphery always uses OnboardingToken** — Added `AtomicBool` to track
   onboarding completion within process lifetime; after first successful
   onboarding, switches to `EndpointId` login for reconnections.
