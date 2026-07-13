# Caddy + DNS Ingress Design Spec

**Date:** 2026-07-12
**Milestone:** M4
**Status:** User-approved, pre-implementation
**PR:** (pending)

## [S1] Problem

M3 replaced Komodo's transport layer with Iroh (QUIC + TLS 1.3, RFC 7250 raw public keys). The control plane (Core↔Periphery communication) now runs entirely over Iroh. However, user-deployed Docker web applications still have no automatic HTTPS ingress. Users deploy containers that bind random high host ports, but there is no reverse proxy to route domain-based HTTP traffic to those ports.

The problem: when a user deploys a web app (e.g., Next.js on port 3000) via Komodo, they manually need to:
- Know the host port Docker assigned
- Set up their own reverse proxy (nginx/Caddy) 
- Obtain TLS certificates
- Create DNS records

This design automates all of that: DNS record creation, TLS certificate issuance, and HTTP reverse proxying — all self-contained within the Luddite platform, using Iroh for the data plane.

## [S2] Solution Overview

Six key decisions form the architecture:

1. **Caddy as the reverse proxy** — Vendored static binary with Cloudflare DNS plugin, running as a host process on designated ingress nodes. Configured via JSON (not Caddyfile) through Caddy's admin API (`POST /load`). No text config generation.

2. **Dedicated ingress nodes** — Only nodes with public IPs run Caddy on ports 80/443. Non-public nodes are "worker" nodes that receive HTTP traffic via Iroh streams from the ingress node. A node declares itself as ingress via config (`ingress_enabled: true` + `public_ipv4`).

3. **Iroh HTTP bridge (data plane)** — In-process axum HTTP listener on the ingress Periphery that opens Iroh QUIC bidi streams (ALPN `luddite/http-proxy/1`) to worker Periphery nodes. Caddy reverse-proxies to this listener on `127.0.0.1:<bridge_port>` with an `X-Target-Endpoint` header. Worker Periphery accepts streams and forwards to Docker container host ports. Self-contained, no Tailscale, no dumbpipe sidecar.

4. **Cloudflare for DNS management** — First implementation of a trait-abstracted `DnsProvider` interface. Cloudflare API creates/updates/deletes A records for app subdomains (`myapp.example.com`). Grey-cloud (`proxied: false`) so Caddy handles TLS. Also used for DNS-01 ACME challenge in Caddy's TLS automation. API token stored in CoreConfig, sent to ingress Periphery via Iroh control plane.

5. **DNS record registry in Core's DB** — Core maintains a `dns_records` table tracking every DNS record created, including which ingress node it points to. Enables ingress node failover: when an ingress node goes down, Core updates all affected DNS records to point to a new ingress node and reloads Caddy on the new node.

6. **Vendored binary pipeline** — Separate `github.com/luddite-dev/vendored` repo with daily CI that checks for new Caddy upstream releases, builds with `xcaddy build` (CGO disabled, static binary), publishes release assets with SHA256 checksums, and updates `manifest.json` on the main branch. Periphery fetches `manifest.json` to detect version changes. Clean separation from Core releases. Extensible for future vendored dependencies.

## [S3] Topology & Traffic Flow

### Node Roles

Every Periphery node is either:

- **Ingress node** — has a public IPv4/IPv6 address, runs Caddy on ports 80/443, terminates TLS, runs the Iroh HTTP bridge listener on `127.0.0.1:<bridge_port>`. Declared via `ServerConfig { ingress_enabled: true, public_ipv4: Some("...") }`.
- **Worker node** — no public IP required. Runs only the Iroh HTTP forward handler (accepts streams on ALPN `luddite/http-proxy/1`, forwards to local Docker container ports).

A node can be both ingress and worker (runs Caddy + serves containers locally — bridge loops back to localhost).

### Traffic Flow (Happy Path)

```
Internet → DNS (app1.example.com → A record → ingress node public IP)
         → Caddy (:443, TLS termination, ACME cert via DNS-01)
         → reverse_proxy 127.0.0.1:<bridge_port> (with X-Target-Endpoint header)
         → Iroh HTTP Bridge (in Periphery process, axum listener)
         → Iroh QUIC stream (ALPN luddite/http-proxy/1) → worker node
         → Worker Periphery accepts stream, reads target container host port
         → HTTP request forwarded to 127.0.0.1:<container_host_port>
         → Response piped back over the same Iroh stream
```

### Local Deployments on Ingress Nodes

When a deployment runs on the ingress node itself, the Iroh HTTP bridge detects that the target endpoint ID is its own endpoint and forwards directly to `127.0.0.1:<container_host_port>` without opening an Iroh stream.

## [S4] DNS Management Layer

### DnsProvider Trait

The DNS layer is trait-abstracted so Cloudflare is the first implementation, not the only possible one:

```rust
#[async_trait]
pub trait DnsProvider: Send + Sync {
    async fn resolve_zone_id(&self, domain: &str) -> Result<String>;
    async fn create_record(&self, zone_id: &str, record_type: RecordType, name: &str, content: &str, ttl: u32) -> Result<String>;
    async fn update_record(&self, zone_id: &str, record_id: &str, content: &str) -> Result<()>;
    async fn delete_record(&self, zone_id: &str, record_id: &str) -> Result<()>;
}
```

First implementation: `CloudflareDnsProvider` in `bin/core/src/dns/cloudflare.rs`. Future: `TechnitiumDnsProvider`, `Rfc2136DnsProvider`.

### DNS Record Lifecycle

| Event | DNS Action |
|--------|-----------|
| App deployed with HTTP proxy enabled | Create A record: `app-name.example.com → ingress_node_IP`. Store in `dns_records` table. |
| App undeployed/deleted | Delete A record from provider. Remove from `dns_records` table. |
| App redeployed to different worker node | No DNS change (DNS points to ingress, not worker). |
| Ingress node fails | Update all affected records to new ingress node IP. |
| Ingress node added | No automatic action (new ingress gets routes only via failover or manual rebalance). |

### Cloudflare Configuration

- A records: `proxied: false` (grey cloud) — Caddy handles TLS end-to-end
- TTL: 60 seconds for managed records (fast failover propagation)
- API token needs: `Zone:Zone:Read` + `Zone:DNS:Edit` on managed zones

### Two Paths Touch Cloudflare

1. **Core's DnsProvider trait** — creates/updates/deletes A/AAAA records for app routing (Rust)
2. **Caddy's DNS-01 plugin** — creates/deletes `_acme-challenge` TXT records for cert issuance (Go, baked into vendored Caddy binary)

Both use the same Cloudflare API token (sent from Core to Periphery via Iroh control plane).

## [S5] Ingress Node Failover

### DNS Record Registry

Core maintains a `dns_records` table:

```
DnsRecord {
    id: String,                          // UUID
    record_type: DnsRecordType,          // A, AAAA
    hostname: String,                     // "app1.example.com"
    target_node_id: String,               // ingress node ID (not IP)
    provider_type: String,                // "cloudflare" (future: "technitium", "rfc2136")
    provider_zone_id: String,             // provider's zone ID
    provider_record_id: String,           // provider's record ID (for updates/deletes)
    deployment_id: Option<String>,        // which deployment this serves
    ttl: u32,                             // 60 for managed records
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}
```

### Failover Sequence

1. Core detects ingress node N1 is down (`ServerState → NotOk`)
2. Core selects new ingress node N2 (`ingress_enabled: true`, `ServerState::Ok`)
3. If no other ingress node available → Core logs alert, traffic fails until ingress returns or is manually added
4. For each `DnsRecord` where `target_node_id = N1`:
   a. Update DNS record: IP changes from `N1.public_ipv4` → `N2.public_ipv4` (via `DnsProvider::update_record`)
   b. Update `DnsRecord.target_node_id = N2` in DB
5. Core sends N2 the complete Caddy JSON config (all routes for migrated deployments)
6. N2's Peribility writes JSON config, calls Caddy admin API `POST /load` (hot reload)
7. Traffic flows: Internet → N2 (Caddy) → Iroh → worker nodes

### Key Properties

- **Iroh endpoint-ID-based routing** means no Iroh topology change needed — the bridge on N2 dials the same worker endpoint ID regardless of which ingress node it runs on.
- **No auto-migrate-back.** When N1 returns to `Ok`, records stay on N2 until manually rebalanced or N2 goes down. Prevents DNS churn.
- **DNS propagation:** Cloudflare updates propagate within seconds. Low TTL (60s) on managed records ensures swift failover.

## [S6] Caddy Layer (TLS, ACME, JSON Config)

### JSON Configuration

Caddy is configured via JSON, not Caddyfile text. Periphery builds JSON from typed Rust structs (serde-serializable), validates, and pushes via Caddy admin API (`POST /load` with `Content-Type: application/json`). If Caddy rejects the JSON (validation error), old config stays running.

### JSON Structure (simplified)

```json
{
  "apps": {
    "http": {
      "servers": {
        "main": {
          "listen": [":80", ":443"],
          "routes": [
            {
              "match": [{"host": ["app1.example.com"]}],
              "handle": [{
                "handler": "reverse_proxy",
                "upstreams": [{"dial": "127.0.0.1:8443"}],
                "headers": {
                  "request": {
                    "set": {
                      "X-Target-Endpoint": ["<worker_endpoint_id>"]
                    }
                  }
                }
              }]
            }
          ]
        }
      }
    },
    "tls": {
      "automation": {
        "policies": [{
          "issuers": [{
            "module": "acme",
            "challenges": {
              "dns": {
                "provider": {
                  "name": "cloudflare",
                  "api_token": "<literal_token_from_core>"
                }
              }
            }
          }]
        }]
      }
    }
  }
}
```

Key decisions:
- **All apps reverse_proxy to same localhost port** (`127.0.0.1:8443`) — bridge distinguishes targets via `X-Target-Endpoint` header
- **Explicit cert issuance** (not on-demand TLS) — Core controls domain creation, so hostnames listed explicitly. No `on_demand_tls` `ask` endpoint needed.
- **DNS-01 via `acme_dns cloudflare`** — wildcard-capable, no port 80 needed for ACME
- **Port 80** — Caddy listens for HTTP→HTTPS redirects only (no app served on HTTP)

### API Token Injection

- Token stored in `CoreConfig` (pattern: `file:/config/cloudflare-token`)
- Core sends token to ingress Periphery via Iroh control plane (same secure channel as all Core→Periphery communication)
- Periphery injects token as literal value in Caddy JSON config's `tls.automation` section (not env var)
- Token never written to disk as env var on Periphery
- Token rotation: Core pushes new config → Periphery rebuilds + reloads Caddy JSON

### Caddyfile Reload Mechanism

- Caddy admin endpoint on `127.0.0.1:2019` (localhost only, for security)
- Periphery writes new JSON config, POSTs to `/load` (hot reload, zero downtime)
- `admin off` must NOT be set — it disables hot reload

## [S7] Iroh HTTP Bridge (Data Plane)

### New ALPN: `luddite/http-proxy/1`

Separate from the control plane ALPN (`luddite/control/1`). Separation ensures:
- Control-plane traffic never mixes with proxied HTTP traffic
- Different stream handling logic (control: `TransportMessage` framing; HTTP: raw HTTP pipe)
- Independent rate-limiting/security possible

### Ingress Side (Bridge Listener)

On the ingress Periphery, a local axum HTTP listener runs on `127.0.0.1:<bridge_port>` (default 8443):

1. Caddy sends HTTP request to `127.0.0.1:8443` with `X-Target-Endpoint: <worker_endpoint_id>` header
2. Bridge extracts endpoint ID from header
3. **Local shortcut:** If target endpoint ID is the ingress node's own endpoint → forward directly to `127.0.0.1:<container_port>` (no Iroh stream)
4. **Remote forwarding:** Otherwise, look up or open pooled Iroh connection to target worker
5. Open new bidi stream on that connection (ALPN `luddite/http-proxy/1`)
6. Write target host port (u16) as stream prefix
7. Pipe raw HTTP request bytes into stream's send half
8. Read raw HTTP response from stream's recv half, relay back to Caddy

### Worker Side (Forward Handler)

On the worker Periphery, the Iroh endpoint accepts streams on `luddite/http-proxy/1`:

1. Accept bidi stream
2. Read `target_host_port` (u16)
3. Read raw HTTP request bytes
4. Forward to `127.0.0.1:<target_host_port>` (Docker container's published host port)
5. Pipe raw HTTP response back over stream's send half

### Stream Protocol

```
[u16 target_host_port][raw HTTP/1.1 request bytes...][half-close (fin) = request complete]
[raw HTTP/1.1 response bytes...][half-close (fin) = response complete]
```

No full HTTP parsing — raw byte piping. Half-close (fin) signals completion, same as TCP.

### Connection Pooling

- One persistent QUIC connection per worker endpoint ID (maintained by ingress Periphery)
- Each HTTP request opens a new bidi stream on existing connection (QUIC streams are cheap — no handshake per stream)
- Connections established lazily, kept alive indefinitely (QUIC keepalive handles liveness)

### Target Port Source

From existing `host_ports` data contract (M1 placement design):
- Periphery's `ReadContainerPorts` returns assigned host ports
- Core stores in `DeploymentInfo.host_ports`
- When generating Caddy JSON, Core maps deployment's HTTP port to worker's endpoint ID

Mapping: `hostname → deployment_id → assigned_server (worker) → worker_endpoint_id + container_HTTP_port`

**Prerequisite:** The `TODO(Task 8)` at `bin/core/src/resource/deployment.rs:216` and `bin/core/src/resource/stack.rs:286` must be implemented — `ReadContainerPorts` readback to populate `info.host_ports` on normal create/update (not just migration).

## [S8] Vendored Binary Pipeline

### Repository: `github.com/luddite-dev/vendored`

Separate repo with daily CI, clean separation from Core releases. Extensible for future vendored dependencies.

### Structure

```
luddite-dev/vendored/
├── .github/workflows/
│   ├── caddy-check.yml       # Daily: checks for new Caddy release
│   └── caddy-build.yml       # Builds with xcaddy + cloudflare plugin
├── manifest.json             # Version manifest (updated by CI after each release)
└── README.md
```

### manifest.json Format

```json
{
  "version": 1,
  "artifacts": {
    "caddy": {
      "version": "2.9.1",
      "upstream_version": "v2.9.1",
      "plugins": [
        { "name": "cloudflare-dns", "module": "github.com/caddy-dns/cloudflare", "version": "v0.0.0-..." }
      ],
      "checksums": {
        "linux-amd64": "sha256:abc123...",
        "linux-arm64": "sha256:def456..."
      },
      "download_url": "https://github.com/luddite-dev/vendored/releases/download/caddy-2.9.1/caddy-luddite-2.9.1-linux-{{arch}}"
    }
  }
}
```

### CI Workflow

1. `caddy-check.yml` runs daily at a randomized time
2. Fetches latest Caddy release tag from `github.com/caddyserver/caddy`
3. Compares against `manifest.json["artifacts"]["caddy"]["version"]`
4. If newer → triggers `caddy-build.yml`
5. Build workflow: `xcaddy build v<version> --with github.com/caddy-dns/cloudflare@<plugin_version>` (no main.go needed — xcaddy generates it internally)
6. Cross-compile: linux-amd64, linux-arm64
7. Upload as GitHub release asset
8. Update `manifest.json` with new version + checksums
9. Commit `manifest.json` back to `main`

### Periphery Version Awareness

- Periphery fetches `manifest.json` from `https://raw.githubusercontent.com/luddite-dev/vendored/main/manifest.json`
- Compares local Caddy version against manifest
- If mismatch: downloads binary, verifies SHA256, swaps, restarts Caddy
- Check happens on Periphery startup and periodically (hourly, configurable)

### Binary Path

Default: `~/.local/share/luddite/bin/caddy-luddite` (NOT `/usr/local/bin` — that requires root to rw).

### Process Supervision

- Periphery spawns Caddy as child process: `caddy run --config <path> --adapter json`
- Caddy admin endpoint (`127.0.0.1:2019`) used for config pushes
- Periphery monitors Caddy health; restarts on crash
- On Periphery shutdown: graceful Caddy stop

### Binary Updates (vs Config Reloads)

- **Config changes** (new app, route update, token rotation): hot reload via `POST /load` (zero downtime)
- **Binary updates** (security patches, plugin upgrades): process restart (brief downtime, a few seconds). Rare, can be scheduled.

## [S9] Config & Entity Changes

### CoreConfig Additions

```rust
pub struct CoreConfig {
    // ... existing fields ...
    pub ingress: IngressConfig,
}

pub struct IngressConfig {
    pub dns: DnsProviderConfig,
}

pub struct DnsProviderConfig {
    pub provider: String,              // "cloudflare" (future: "technitium", "rfc2136")
    pub cloudflare_api_token: Option<String>,  // file:/config/cloudflare-token
    pub base_domain: Option<String>,   // "example.com" → "app1.example.com"
}
```

### ServerConfig Additions

```rust
pub struct ServerConfig {
    // ... existing fields ...
    pub ingress_enabled: bool,              // is this an ingress node?
    pub public_ipv4: Option<String>,       // required if ingress_enabled
    pub public_ipv6: Option<String>,       // optional, additional
}
```

### DeploymentConfig Additions

```rust
pub struct DeploymentConfig {
    // ... existing fields ...
    pub http_proxy: Option<HttpProxyConfig>,
}

pub struct HttpProxyConfig {
    pub subdomain: String,      // "myapp" → "myapp.example.com"
    pub container_port: u16,    // which container port to proxy to
}
```

### PeripheryConfig Additions

```rust
pub struct PeripheryConfig {
    // ... existing fields ...
    pub http_bridge_port: u16,           // default 8443, ingress nodes only
    pub caddy_binary_path: String,        // default ~/.local/share/luddite/bin/caddy-luddite
    pub vendored_manifest_url: String,    // default https://raw.githubusercontent.com/luddite-dev/vendored/main/manifest.json
}
```

### New Entity: DnsRecord

```rust
pub struct DnsRecord {
    pub id: String,
    pub record_type: DnsRecordType,       // A, AAAA
    pub hostname: String,                // "myapp.example.com"
    pub target_node_id: String,           // ingress node ID
    pub provider_type: String,             // "cloudflare"
    pub provider_zone_id: String,          // provider zone ID
    pub provider_record_id: String,       // provider record ID
    pub deployment_id: Option<String>,
    pub ttl: u32,                          // 60
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub enum DnsRecordType { A, AAAA }
```

### New Module: `bin/core/src/dns/`

```
bin/core/src/dns/
├── mod.rs             // module declarations, build_dns_provider()
├── provider.rs        // DnsProvider trait
└── cloudflare.rs      // CloudflareDnsProvider implementation
```

### New Module: `bin/periphery/src/caddy/`

```
bin/periphery/src/caddy/
├── mod.rs             // module declarations, Caddy supervisor
├── config.rs          // JSON config builder (serde structs)
└── supervisor.rs      // process lifecycle, admin API client
```

### New Module: `bin/periphery/src/http_bridge/`

```
bin/periphery/src/http_bridge/
├── mod.rs             // module declarations
├── ingress.rs         // axum listener on ingress node
└── forward.rs         // stream handler on worker node
```

## [S10] End-to-End Data Flow

### Deployment Creation with HTTP Proxy

1. User creates deployment with `http_proxy: { subdomain: "myapp", container_port: 3000 }`
2. Core's placement scheduler assigns deployment to worker node N2
3. Core selects an ingress node (N1, `ingress_enabled: true`, `ServerState::Ok`)
4. Core creates DNS A record: `myapp.example.com → N1.public_ipv4` (via `DnsProvider::create_record`)
5. Core stores `DnsRecord` in DB (`hostname`, `target_node_id=N1`, `deployment_id`, provider IDs)
6. Core sends ingress config update to N1's Periphery via Iroh control plane:
   - Caddy JSON route for `myapp.example.com` (with `X-Target-Endpoint: <N2_endpoint_id>`)
   - Cloudflare API token (for Caddy's DNS-01 challenge)
7. N1's Periphery:
   - Downloads/verifies Caddy binary if needed (checks `manifest.json`)
   - Builds Caddy JSON config (all routes including new one)
   - `POST /load` to Caddy admin API (hot reload)
   - HTTP bridge listener ready to forward `myapp.example.com` → N2
8. Browser → `myapp.example.com` → DNS → N1:443 (Caddy TLS) → `127.0.0.1:8443` (bridge)
   → Iroh stream (ALPN `luddite/http-proxy/1`) → N2 → `127.0.0.1:<container_port>` → response

### DNS Record TTL

Cloudflare managed records: TTL = 60s for fast failover propagation.

## [S11] Verification

### Build Verification
- `cargo check --workspace` — 0 errors, 0 warnings
- `cargo test -p transport --lib` — existing tests still pass
- `cargo fmt -- --check` — pass

### Integration Verification
- Deploy a test container with HTTP proxy on a worker node
- Verify DNS A record created in Cloudflare
- Verify TLS cert obtained (DNS-01 challenge succeeds)
- Verify HTTP request to `myapp.example.com` reaches the container
- Verify HTTPS redirect works (HTTP→HTTPS)
- Verify Caddy admin API hot reload works (add/remove app without downtime)

### Failover Verification
- Stop Caddy/Periphery on ingress node N1
- Verify Core detects `ServerState → NotOk`
- Verify DNS records updated to new ingress node N2
- Verify Caddy on N2 reloaded with migrated routes
- Verify traffic flows through N2

### Vendoring Verification
- Verify `manifest.json` fetch + checksum validation
- Verify Caddy binary download and binary swap
- Verify Caddy process starts and admin API is reachable

## [S12] Out of Scope

- **Custom domains (bring your own domain)** — future. Would require On-Demand TLS with `ask` endpoint. Currently only subdomains under a managed base domain.
- **Cloudflare proxy (orange cloud)** — future config option. Currently grey-cloud only.
- **Auto-rebalance back to recovered ingress nodes** — no auto-migrate-back. Manual rebalancing only.
- **Non-Cloudflare DNS providers** — trait-abstracted, but only CloudflareDnsProvider is implemented now.
- **WebSocket proxying through the bridge** — needs testing. Raw byte piping should work for WebSocket upgrades, but needs explicit verification.
- **Multi-ingress load balancing** — single ingress node per deployment. No DNS-level load balancing across multiple ingress IPs.
- **Core's own HTTP API behind Caddy** — separate from this design. Core's API (:9120 axum server) remains behind operator-managed proxy or direct port exposure.
