# Caddy + DNS Ingress Implementation Plan

**Status:** ✅ All 11 tasks complete + e2e tested. PR: https://github.com/luddite-dev/deploy/pull/18

> **For agentic workers:** REQUIRED SUB-SKILL: Use compose:subagent (recommended) or compose:execute to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement automatic HTTPS ingress for user-deployed Docker web apps using Caddy reverse proxy, Cloudflare DNS management, and an Iroh-based HTTP bridge from ingress nodes to worker nodes.

**Architecture:** Dedicated ingress nodes (public IP) run Caddy on 80/443 + an in-process Iroh HTTP bridge listener. Worker nodes accept Iroh streams and forward to local Docker container ports. Core manages DNS records via a trait-abstracted `DnsProvider` (Cloudflare first), maintains a `dns_records` DB table for failover tracking, and pushes Caddy JSON config to ingress Peripheries via the Iroh control plane. Caddy is vendored as a static binary via a dedicated `luddite-dev/vendored` repo with daily CI.

**Tech Stack:** Rust (axum, iroh, reqwest, serde_json), Caddy (xcaddy-built with caddy-dns/cloudflare), Cloudflare API, MongoDB (existing)

## Global Constraints

- `CARGO_TARGET_DIR=/home/acheong/.cargo-target` before every cargo command
- `cargo fmt` before commits (enforced by `.githooks/pre-commit`)
- Build with `cargo build -p komodo_core -p komodo_periphery` (package names, not binary names)
- Hard fork — no backward compatibility constraints, all types can be freely broken
- Vendored Caddy binary path: `~/.local/share/luddite/bin/caddy-luddite` (NOT `/usr/local/bin`)
- DNS provider must be trait-abstracted (`DnsProvider` trait), Cloudflare is first impl only
- Caddy configured via JSON (serde structs + `POST /load` admin API), NOT Caddyfile text
- Spec: `docs/compose/specs/2026-07-12-caddy-dns-ingress-design.md`

---

## File Structure

### New files (Core side)

```
bin/core/src/dns/
├── mod.rs              # module declarations, build_dns_provider()
├── provider.rs         # DnsProvider trait, RecordType enum
└── cloudflare.rs       # CloudflareDnsProvider implementation

bin/core/src/ingress/
├── mod.rs              # module declarations
├── config.rs           # Caddy JSON config builder (serde structs)
├── failover.rs         # ingress node failover logic
└── management.rs       # deployment→DNS→Caddy config orchestration
```

### New files (Periphery side)

```
bin/periphery/src/caddy/
├── mod.rs              # module declarations
├── supervisor.rs       # Caddy process lifecycle, admin API client
└── binary.rs           # vendored binary download + manifest check

bin/periphery/src/http_bridge/
├── mod.rs              # module declarations
├── ingress.rs          # axum listener on ingress node
└── forward.rs          # stream handler on worker node
```

### New entity files

```
client/core/rs/src/entities/dns.rs   # DnsRecord, DnsRecordType, DnsProviderConfig, etc.
```

### Modified files

- `client/core/rs/src/entities/config/core.rs` — add `IngressConfig` field to `CoreConfig`
- `client/core/rs/src/entities/server.rs` — add `ingress_enabled`, `public_ipv4`, `public_ipv6` to `ServerConfig`
- `client/core/rs/src/entities/deployment.rs` — add `http_proxy: Option<HttpProxyConfig>` to `DeploymentConfig`
- `client/core/rs/src/entities/config/periphery.rs` — add `http_bridge_port`, `caddy_binary_path`, `vendored_manifest_url`
- `bin/core/src/config.rs` — add `cloudflare_api_token()` loader
- `bin/core/src/main.rs` — initialize DNS provider
- `bin/periphery/src/main.rs` — start Caddy supervisor (ingress nodes) + HTTP bridge
- `bin/periphery/src/state.rs` — add ingress config accessors
- `bin/core/src/resource/deployment.rs` — wire DNS record create/delete + Caddy config sync on deploy/undeploy
- `bin/core/src/resource/deployment.rs:216` — implement `TODO(Task 8)` ReadContainerPorts readback
- `bin/core/src/resource/stack.rs:286` — implement `TODO(Task 8)` ReadContainerPorts readback (stack equivalent)
- `Cargo.toml` (workspace) — add `reqwest` features if needed (likely already available)
- `bin/core/Cargo.toml` — add `async-trait`
- `bin/periphery/Cargo.toml` — add dependencies for Caddy admin client

### Vendored repo (separate)

```
github.com/luddite-dev/vendored/
├── .github/workflows/
│   ├── caddy-check.yml
│   └── caddy-build.yml
├── manifest.json
└── README.md
```

---

## Task 1: DNS Provider Trait + Cloudflare Implementation

**Covers:** [S4]

**Files:**
- Create: `bin/core/src/dns/mod.rs`
- Create: `bin/core/src/dns/provider.rs`
- Create: `bin/core/src/dns/cloudflare.rs`
- Create: `client/core/rs/src/entities/dns.rs`
- Modify: `bin/core/Cargo.toml` (add `async-trait`)
- Modify: `bin/core/src/main.rs` (add `mod dns;`)

**Interfaces:**
- Produces: `DnsProvider` trait, `RecordType` enum, `CloudflareDnsProvider` struct, `DnsRecord` / `DnsRecordType` / `DnsProviderConfig` / `IngressConfig` entities

- [ ] **Step 1: Create the DNS entity types**

Create `client/core/rs/src/entities/dns.rs`:

```rust
use typeshare::typeshare;
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

#[typeshare]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "UPPERCASE")]
pub enum DnsRecordType {
  A,
  AAAA,
}

#[typeshare]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct DnsRecord {
  /// UUID
  pub id: String,
  /// Record type (A or AAAA)
  pub record_type: DnsRecordType,
  /// Full hostname, e.g. "app1.example.com"
  pub hostname: String,
  /// The ingress node ID this record points to
  pub target_node_id: String,
  /// DNS provider type, e.g. "cloudflare"
  pub provider_type: String,
  /// Provider's zone ID
  pub provider_zone_id: String,
  /// Provider's record ID (for updates/deletes)
  pub provider_record_id: String,
  /// The deployment this record serves (for cleanup)
  pub deployment_id: Option<String>,
  /// TTL in seconds (60 for managed records)
  pub ttl: u32,
  pub created_at: DateTime<Utc>,
  pub updated_at: DateTime<Utc>,
}

#[typeshare]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct DnsProviderConfig {
  /// Provider type: "cloudflare" (future: "technitium", "rfc2136")
  #[serde(default)]
  pub provider: String,
  /// Cloudflare API token (file: spec or direct value)
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub cloudflare_api_token: Option<String>,
  /// Base domain for app subdomains (e.g. "example.com" → "app1.example.com")
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub base_domain: Option<String>,
}

#[typeshare]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct IngressConfig {
  #[serde(default)]
  pub dns: DnsProviderConfig,
}
```

- [ ] **Step 2: Add IngressConfig to CoreConfig**

In `client/core/rs/src/entities/config/core.rs`, add to the `CoreConfig` struct (after the `secrets` field, before `ssl_enabled`):

```rust
  // ============
  // = Ingress =
  // ============
  /// Ingress / DNS management configuration
  #[serde(default)]
  pub ingress: komodo_client::entities::dns::IngressConfig,
```

Add the import at the top of the file if not already present (entities are typically re-exported via `komodo_client::entities::`).

Add to the `Default` impl for `CoreConfig`:

```rust
      ingress: Default::default(),
```

- [ ] **Step 3: Create the DnsProvider trait**

Create `bin/core/src/dns/provider.rs`:

```rust
use async_trait::async_trait;
use anyhow::Result;

use crate::entities::dns::DnsRecordType;

#[async_trait]
pub trait DnsProvider: Send + Sync {
  /// Resolve a domain to its zone ID (cached internally)
  async fn resolve_zone_id(&self, domain: &str) -> Result<String>;

  /// Create an A or AAAA record, return the provider's record ID
  async fn create_record(
    &self,
    zone_id: &str,
    record_type: DnsRecordType,
    name: &str,
    content: &str,
    ttl: u32,
  ) -> Result<String>;

  /// Update an existing record's content (e.g. IP change for failover)
  async fn update_record(
    &self,
    zone_id: &str,
    record_id: &str,
    content: &str,
  ) -> Result<()>;

  /// Delete a record
  async fn delete_record(
    &self,
    zone_id: &str,
    record_id: &str,
  ) -> Result<()>;
}
```

- [ ] **Step 4: Create the CloudflareDnsProvider implementation**

Create `bin/core/src/dns/cloudflare.rs`:

```rust
use std::sync::Arc;
use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::Client;
use tokio::sync::RwLock;

use crate::entities::dns::DnsRecordType;
use super::provider::DnsProvider;

const CLOUDFLARE_API_BASE: &str = "https://api.cloudflare.com/client/v4";

pub struct CloudflareDnsProvider {
  api_token: String,
  client: Client,
  zone_cache: Arc<RwLock<HashMap<String, String>>>,
}

impl CloudflareDnsProvider {
  pub fn new(api_token: String) -> Result<Self> {
    let client = Client::builder()
      .timeout(Duration::from_secs(30))
      .build()
      .context("Failed to build HTTP client")?;
    Ok(Self {
      api_token,
      client,
      zone_cache: Arc::new(RwLock::new(HashMap::new())),
    })
  }

  fn record_type_str(t: DnsRecordType) -> &'static str {
    match t {
      DnsRecordType::A => "A",
      DnsRecordType::AAAA => "AAAA",
    }
  }
}

#[async_trait]
impl DnsProvider for CloudflareDnsProvider {
  async fn resolve_zone_id(&self, domain: &str) -> Result<String> {
    // Check cache
    {
      let cache = self.zone_cache.read().await;
      if let Some(zone_id) = cache.get(domain) {
        return Ok(zone_id.clone());
      }
    }

    // Query Cloudflare API
    let resp = self
      .client
      .get(format!("{CLOUDFLARE_API_BASE}/zones"))
      .header("Authorization", format!("Bearer {}", self.api_token))
      .query(&[("name", domain)])
      .send()
      .await
      .context("Failed to query Cloudflare zones")?
      .json::<serde_json::Value>()
      .await
      .context("Failed to parse Cloudflare zone response")?;

    let zone_id = resp
      .get("result")
      .and_then(|r| r.as_array())
      .and_then(|arr| arr.first())
      .and_then(|z| z.get("id"))
      .and_then(|id| id.as_str())
      .context(format!("Zone not found for domain: {domain}"))?
      .to_string();

    // Cache
    {
      let mut cache = self.zone_cache.write().await;
      cache.insert(domain.to_string(), zone_id.clone());
    }

    Ok(zone_id)
  }

  async fn create_record(
    &self,
    zone_id: &str,
    record_type: DnsRecordType,
    name: &str,
    content: &str,
    ttl: u32,
  ) -> Result<String> {
    let body = serde_json::json!({
      "type": Self::record_type_str(record_type),
      "name": name,
      "content": content,
      "ttl": ttl,
      "proxied": false,
    });

    let resp = self
      .client
      .post(format!(
        "{CLOUDFLARE_API_BASE}/zones/{zone_id}/dns_records"
      ))
      .header("Authorization", format!("Bearer {}", self.api_token))
      .json(&body)
      .send()
      .await
      .context("Failed to create DNS record")?;

    let json: serde_json::Value =
      resp.json().await.context("Failed to parse response")?;

    if json.get("success").and_then(|s| s.as_bool()) != Some(true) {
      let errors = serde_json::to_string_pretty(&json["errors"]).unwrap_or_default();
      bail!("Cloudflare API error creating record: {errors}");
    }

    let record_id = json
      .get("result")
      .and_then(|r| r.get("id"))
      .and_then(|id| id.as_str())
      .context("Missing record ID in response")?
      .to_string();

    Ok(record_id)
  }

  async fn update_record(
    &self,
    zone_id: &str,
    record_id: &str,
    content: &str,
  ) -> Result<()> {
    let body = serde_json::json!({
      "content": content,
    });

    let resp = self
      .client
      .patch(format!(
        "{CLOUDFLARE_API_BASE}/zones/{zone_id}/dns_records/{record_id}"
      ))
      .header("Authorization", format!("Bearer {}", self.api_token))
      .json(&body)
      .send()
      .await
      .context("Failed to update DNS record")?;

    let json: serde_json::Value =
      resp.json().await.context("Failed to parse response")?;

    if json.get("success").and_then(|s| s.as_bool()) != Some(true) {
      let errors = serde_json::to_string_pretty(&json["errors"]).unwrap_or_default();
      bail!("Cloudflare API error updating record: {errors}");
    }

    Ok(())
  }

  async fn delete_record(
    &self,
    zone_id: &str,
    record_id: &str,
  ) -> Result<()> {
    let resp = self
      .client
      .delete(format!(
        "{CLOUDFLARE_API_BASE}/zones/{zone_id}/dns_records/{record_id}"
      ))
      .header("Authorization", format!("Bearer {}", self.api_token))
      .send()
      .await
      .context("Failed to delete DNS record")?;

    let json: serde_json::Value =
      resp.json().await.context("Failed to parse response")?;

    if json.get("success").and_then(|s| s.as_bool()) != Some(true) {
      let errors = serde_json::to_string_pretty(&json["errors"]).unwrap_or_default();
      bail!("Cloudflare API error deleting record: {errors}");
    }

    Ok(())
  }
}
```

- [ ] **Step 5: Create the dns module mod.rs**

Create `bin/core/src/dns/mod.rs`:

```rust
pub mod provider;
pub mod cloudflare;

use anyhow::Result;
use std::sync::Arc;

use provider::DnsProvider;
use cloudflare::CloudflareDnsProvider;

use crate::config::core_config;

/// Build a DNS provider from CoreConfig.
/// Returns None if DNS provider is not configured.
pub fn build_dns_provider() -> Result<Option<Arc<dyn DnsProvider>>> {
  let config = core_config();
  let provider_type = &config.ingress.dns.provider;

  if provider_type.is_empty() {
    return Ok(None);
  }

  match provider_type.as_str() {
    "cloudflare" => {
      let token_spec = config
        .ingress
        .dns
        .cloudflare_api_token
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("cloudflare_api_token not configured"))?;

      let token = if let Some(stripped) = token_spec.strip_prefix("file:") {
        mogh_secret_file::maybe_read_item_from_file(stripped)?
      } else {
        token_spec.to_string()
      };

      let provider = CloudflareDnsProvider::new(token)?;
      Ok(Some(Arc::new(provider)))
    }
    other => Err(anyhow::anyhow!(
      "Unknown DNS provider type: {other}. Supported: cloudflare"
    )),
  }
}
```

- [ ] **Step 6: Add module to main.rs and Cargo.toml**

In `bin/core/src/main.rs`, add after the existing `mod` declarations:

```rust
mod dns;
```

In `bin/core/Cargo.toml`, add to `[dependencies]`:

```toml
async-trait = "0.1"
```

- [ ] **Step 7: Add the dns entity module to entities**

In `client/core/rs/src/entities/mod.rs` (or wherever entities are re-exported), add:

```rust
pub mod dns;
```

If there is a `lib.rs` pattern, follow the same pattern used by existing entity modules like `server`.

- [ ] **Step 8: Build and verify compilation**

Run:
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo check -p komodo_core -p komodo_client 2>&1 | tail -20
```
Expected: 0 errors (warnings about unused code are OK at this stage).

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(core): add DnsProvider trait + Cloudflare implementation

Trait-abstracted DNS provider with Cloudflare as first implementation.
Supports create/update/delete of A and AAAA records via Cloudflare API.
DnsRecord entity, DnsProviderConfig, IngressConfig entities added to
CoreConfig. Zone ID caching. Token loaded via file: spec pattern."
```

---

## Task 2: Server Config Ingress Fields + Periphery Config

**Covers:** [S3, S9]

**Files:**
- Modify: `client/core/rs/src/entities/server.rs` (add `ingress_enabled`, `public_ipv4`, `public_ipv6` to `ServerConfig`)
- Modify: `client/core/rs/src/entities/config/periphery.rs` (add `http_bridge_port`, `caddy_binary_path`, `vendored_manifest_url`)
- Modify: `bin/periphery/src/state.rs` (add config accessors)

**Interfaces:**
- Consumes: `IngressConfig` from Task 1
- Produces: `ServerConfig.ingress_enabled`, `ServerConfig.public_ipv4/v6`, `PeripheryConfig.http_bridge_port/caddy_binary_path/vendored_manifest_url`

- [ ] **Step 1: Add ingress fields to ServerConfig**

In `client/core/rs/src/entities/server.rs`, add to the `ServerConfig` struct (after `maintenance_windows`):

```rust
  // =============
  // = Ingress =
  // =============
  /// Whether this node is an ingress node (runs Caddy on 80/443).
  /// Default: false
  #[serde(default)]
  #[builder(default)]
  pub ingress_enabled: bool,

  /// Public IPv4 address for ingress traffic.
  /// Required if ingress_enabled is true.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  #[builder(default)]
  pub public_ipv4: Option<String>,

  /// Public IPv6 address for ingress traffic. Optional, additional.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  #[builder(default)]
  pub public_ipv6: Option<String>,
```

Add to the `Default` impl for `ServerConfig`:

```rust
      ingress_enabled: false,
      public_ipv4: None,
      public_ipv6: None,
```

- [ ] **Step 2: Add ingress fields to PeripheryConfig**

In `client/core/rs/src/entities/config/periphery.rs`, add to the `PeripheryConfig` struct (after the `default_terminal_command` section or similar, before any closing brace):

```rust
  // =================
  // = HTTP BRIDGE =
  // =================
  /// Port for the Iroh HTTP bridge listener (ingress nodes only).
  /// Caddy reverse_proxies to this localhost port.
  /// Default: 8443
  #[serde(default = "default_http_bridge_port")]
  pub http_bridge_port: u16,

  /// Path to the vendored Caddy binary (ingress nodes only).
  /// Default: ~/.local/share/luddite/bin/caddy-luddite
  #[serde(default = "default_caddy_binary_path")]
  pub caddy_binary_path: String,

  /// URL to the vendored manifest.json for version checks.
  /// Default: https://raw.githubusercontent.com/luddite-dev/vendored/main/manifest.json
  #[serde(default = "default_vendored_manifest_url")]
  pub vendored_manifest_url: String,
```

Add the default functions:

```rust
fn default_http_bridge_port() -> u16 {
  8443
}

fn default_caddy_binary_path() -> String {
  let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
  format!("{home}/.local/share/luddite/bin/caddy-luddite")
}

fn default_vendored_manifest_url() -> String {
  "https://raw.githubusercontent.com/luddite-dev/vendored/main/manifest.json".to_string()
}
```

- [ ] **Step 3: Build and verify**

Run:
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo check --workspace 2>&1 | tail -10
```
Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat: add ingress config fields to ServerConfig + PeripheryConfig

ServerConfig gains ingress_enabled, public_ipv4, public_ipv6.
PeripheryConfig gains http_bridge_port (8443), caddy_binary_path
(~/.local/share/luddite/bin/caddy-luddite), vendored_manifest_url."
```

---

## Task 3: Deployment HttpProxyConfig

**Covers:** [S9]

**Files:**
- Modify: `client/core/rs/src/entities/deployment.rs` (add `HttpProxyConfig` + `http_proxy` field to `DeploymentConfig`)

**Interfaces:**
- Produces: `HttpProxyConfig { subdomain: String, container_port: u16 }`, `DeploymentConfig.http_proxy`

- [ ] **Step 1: Add HttpProxyConfig struct**

In `client/core/rs/src/entities/deployment.rs`, add near the other config structs (e.g., near `PortMapping`):

```rust
#[typeshare]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct HttpProxyConfig {
  /// Subdomain for this app (e.g. "myapp" → "myapp.example.com")
  pub subdomain: String,
  /// Which container port to proxy HTTP traffic to
  pub container_port: u16,
}
```

- [ ] **Step 2: Add http_proxy field to DeploymentConfig**

In the same file, add to the `DeploymentConfig` struct:

```rust
  /// HTTP ingress configuration for this deployment.
  /// When set, creates DNS record + Caddy route for automatic HTTPS.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  #[builder(default)]
  pub http_proxy: Option<HttpProxyConfig>,
```

- [ ] **Step 3: Build and verify**

Run:
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo check --workspace 2>&1 | tail -10
```
Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat: add HttpProxyConfig to DeploymentConfig

DeploymentConfig gains optional http_proxy field with subdomain
and container_port. When set, triggers DNS record creation and
Caddy route generation for automatic HTTPS."
```

---

## Task 4: ReadContainerPorts Readback (Task 8 Prerequisite)

**Covers:** [S7] (prerequisite — host_ports data contract)

**Files:**
- Modify: `bin/core/src/resource/deployment.rs:216` (implement TODO Task 8)
- Modify: `bin/core/src/resource/stack.rs:286` (implement TODO Task 8)

**Interfaces:**
- Consumes: `ReadContainerPorts` RPC from `periphery_client::api::placement`
- Produces: populated `DeploymentInfo.host_ports` on create/update (not just migration)

- [ ] **Step 1: Read the existing TODO and nearby code**

Read `bin/core/src/resource/deployment.rs` around line 216 to understand what the TODO expects. Read `bin/core/src/resource/stack.rs` around line 286. Read the existing drain path (`bin/core/src/server/drain.rs:325-373`) to see how `ReadContainerPorts` is called there — this is the pattern to replicate.

- [ ] **Step 2: Implement ReadContainerPorts readback in deployment create**

In `bin/core/src/resource/deployment.rs`, replace the `// TODO(Task 8)` comment with code that:
1. Gets the `PeripheryClient` for the assigned server
2. Sends `ReadContainerPorts` RPC with the container name
3. Writes the returned `Vec<AssignedPort>` to `info.host_ports`
4. Saves the updated `DeploymentInfo`

Follow the pattern in `bin/core/src/server/drain.rs:325-373`.

- [ ] **Step 3: Implement the equivalent in stack resource**

In `bin/core/src/resource/stack.rs`, replace the `// TODO(Task 8)` comment with the equivalent code for stacks (loop over compose services, read ports for each).

- [ ] **Step 4: Build and verify**

Run:
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo check -p komodo_core 2>&1 | tail -10
```
Expected: 0 errors.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: implement ReadContainerPorts readback on deploy/stack create

Resolves TODO(Task 8). DeploymentInfo.host_ports and StackInfo.host_ports
are now populated on normal create/update path, not just migration.
Required by Caddy controller to discover container host ports."
```

---

## Task 5: Iroh HTTP Bridge (Data Plane)

**Covers:** [S7]

**Files:**
- Create: `bin/periphery/src/http_bridge/mod.rs`
- Create: `bin/periphery/src/http_bridge/ingress.rs`
- Create: `bin/periphery/src/http_bridge/forward.rs`
- Modify: `bin/periphery/src/main.rs` (add module, spawn handlers)
- Modify: `bin/periphery/Cargo.toml` (add dependencies if needed)
- Modify: `lib/transport/src/iroh/endpoint.rs` (add `HTTP_PROXY_ALPN` constant)

**Interfaces:**
- Consumes: `iroh::Endpoint`, `iroh::Connection`, existing transport Iroh infrastructure
- Produces: `start_ingress_bridge(endpoint, port)`, `start_forward_handler(endpoint)`, `HTTP_PROXY_ALPN`

- [ ] **Step 1: Add ALPN constant**

In `lib/transport/src/iroh/endpoint.rs`, add:

```rust
/// ALPN for HTTP proxy streams (data plane).
/// Separate from the control plane ALPN.
pub const HTTP_PROXY_ALPN: &[u8] = b"luddite/http-proxy/1";
```

- [ ] **Step 2: Create the ingress bridge module**

Create `bin/periphery/src/http_bridge/mod.rs`:

```rust
pub mod ingress;
pub mod forward;
```

Create `bin/periphery/src/http_bridge/ingress.rs`:

```rust
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use axum::{
  Router,
  extract::State,
  http::{HeaderMap, Request, StatusCode},
  response::Response,
  routing::any,
};
use iroh::{Connection, EndpointId, EndpointAddr};
use tokio::sync::RwLock;
use tracing::{error, info};

use transport::iroh::endpoint::HTTP_PROXY_ALPN;

/// Connection pool: worker endpoint ID → QUIC connection
type ConnPool = Arc<RwLock<HashMap<String, Connection>>>;

#[derive(Clone)]
pub struct BridgeState {
  pub endpoint: iroh::Endpoint,
  pub pool: ConnPool,
  pub local_endpoint_id: EndpointId,
}

/// Start the HTTP bridge listener on the ingress node.
/// Caddy reverse_proxies to this listener.
pub async fn start_ingress_bridge(
  endpoint: iroh::Endpoint,
  port: u16,
) -> Result<()> {
  let local_endpoint_id = endpoint.endpoint_id();
  let state = BridgeState {
    endpoint,
    pool: Arc::new(RwLock::new(HashMap::new())),
    local_endpoint_id,
  };

  let app = Router::new()
    .route("/{*path}", any(handle_request))
    .with_state(state);

  let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
    .await
    .map_err(|e| anyhow::anyhow!("Failed to bind bridge listener on 127.0.0.1:{port}: {e}"))?;

  info!("HTTP bridge listening on 127.0.0.1:{port}");

  axum::serve(listener, app)
    .await
    .map_err(|e| anyhow::anyhow!("Bridge server error: {e}"))?;

  Ok(())
}

async fn handle_request(
  State(state): State<BridgeState>,
  headers: HeaderMap,
  request: Request<body::Body>,
) -> Response {
  // Extract target endpoint from header
  let target_endpoint = match headers
    .get("X-Target-Endpoint")
    .and_then(|v| v.to_str().ok())
  {
    Some(ep) => ep.to_string(),
    None => {
      return Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(body::Body::from("Missing X-Target-Endpoint header"))
        .unwrap();
    }
  };

  // Local shortcut: if target is self, we don't have the container info here
  // (the ingress node may also be a worker, but its containers are accessed
  //  via the forward handler on the same node). For now, treat all targets
  //  uniformly via Iroh stream — the forward handler on localhost handles
  //  the actual container connection.
  // TODO: optimize local case by skipping Iroh when target == local_endpoint_id.

  // Get or create connection to target
  let conn = match get_or_connect(&state, &target_endpoint).await {
    Ok(c) => c,
    Err(e) => {
      error!("Failed to connect to {target_endpoint}: {e:#}");
      return Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(body::Body::from(format!("Failed to connect: {e:#}")))
        .unwrap();
    }
  };

  // Open bidi stream
  let (mut send, mut recv) = match conn.open_bi().await {
    Ok(s) => s,
    Err(e) => {
      error!("Failed to open bidi stream: {e}");
      return Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(body::Body::from(format!("Stream open failed: {e}")))
        .unwrap();
    }
  };

  // Read the target host port from X-Target-Port header (set by Caddy)
  let target_port: u16 = match headers
    .get("X-Target-Port")
    .and_then(|v| v.to_str().ok())
    .and_then(|s| s.parse().ok())
  {
    Some(p) => p,
    None => {
      return Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(body::Body::from("Missing X-Target-Port header"))
        .unwrap();
    }
  };

  // Write target port as u16 prefix
  if let Err(e) = send.write_all(&target_port.to_be_bytes()).await {
    error!("Failed to write target port: {e}");
    return Response::builder()
      .status(StatusCode::BAD_GATEWAY)
      .body(body::Body::from("Stream write failed"))
      .unwrap();
  }

  // Pipe HTTP request body to stream
  // Read the full request (method, path, headers, body) and pipe as raw bytes
  let (parts, body) = request.into_parts();
  let request_line = format!(
    "{} {} HTTP/1.1\r\n",
    parts.method,
    parts.uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/")
  );

  // Write request line
  if let Err(e) = send.write_all(request_line.as_bytes()).await {
    error!("Failed to write request line: {e}");
    return Response::builder()
      .status(StatusCode::BAD_GATEWAY)
      .body(body::Body::from("Stream write failed"))
      .unwrap();
  }

  // Write headers
  for (name, value) in parts.headers.iter() {
    let header_line = format!(
      "{}: {}\r\n",
      name.as_str(),
      value.to_str().unwrap_or("")
    );
    if let Err(e) = send.write_all(header_line.as_bytes()).await {
      error!("Failed to write header: {e}");
      return Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(body::Body::from("Stream write failed"))
        .unwrap();
    }
  }

  // End of headers
  if let Err(e) = send.write_all(b"\r\n").await {
    error!("Failed to write header terminator: {e}");
    return Response::builder()
      .status(StatusCode::BAD_GATEWAY)
      .body(body::Body::from("Stream write failed"))
      .unwrap();
  }

  // Pipe body
  use axum::body::Body;
  use http_body_util::BodyExt;
  let body_bytes = match body.collect().await {
    Ok(collected) => collected.to_bytes(),
    Err(e) => {
      error!("Failed to read request body: {e}");
      return Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(Body::from("Body read failed"))
        .unwrap();
    }
  };

  if !body_bytes.is_empty() {
    if let Err(e) = send.write_all(&body_bytes).await {
      error!("Failed to write body: {e}");
      return Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(Body::from("Stream write failed"))
        .unwrap();
    }
  }

  // Half-close send to signal request complete
  let _ = send.finish().await;

  // Read response from stream and relay back
  // For now, read all bytes and return as raw HTTP response
  let mut response_buf = Vec::new();
  use tokio::io::AsyncReadExt;
  if let Err(e) = recv.read_to_end(&mut response_buf).await {
    error!("Failed to read response from stream: {e}");
    return Response::builder()
      .status(StatusCode::BAD_GATEWAY)
      .body(Body::from("Stream read failed"))
      .unwrap();
  }

  // Parse the raw HTTP response
  // The forward handler sends back a complete HTTP/1.1 response
  Response::builder()
    .status(StatusCode::OK)
    .body(Body::from(response_buf))
    .unwrap_or_else(|_| {
      Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::from("Failed to build response"))
        .unwrap()
    })
}

async fn get_or_connect(
  state: &BridgeState,
  endpoint_id_str: &str,
) -> Result<Connection> {
  // Check pool
  {
    let pool = state.pool.read().await;
    if let Some(conn) = pool.get(endpoint_id_str) {
      if !conn.is_closed() {
        return Ok(conn.clone());
      }
    }
  }

  // Parse endpoint ID and connect
  let endpoint_id: EndpointId = endpoint_id_str
    .parse()
    .map_err(|e| anyhow::anyhow!("Invalid endpoint ID {endpoint_id_str}: {e}"))?;

  let addr = EndpointAddr::new(endpoint_id);
  let conn = state
    .endpoint
    .connect(addr, HTTP_PROXY_ALPN)
    .await
    .map_err(|e| anyhow::anyhow!("Failed to connect to {endpoint_id_str}: {e}"))?;

  // Store in pool
  {
    let mut pool = state.pool.write().await;
    pool.insert(endpoint_id_str.to_string(), conn.clone());
  }

  Ok(conn)
}
```

- [ ] **Step 3: Create the forward handler module**

Create `bin/periphery/src/http_bridge/forward.rs`:

```rust
use anyhow::Result;
use iroh::Endpoint;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, info, warn};

use transport::iroh::endpoint::HTTP_PROXY_ALPN;

/// Start the HTTP forward handler on the worker node.
/// Accepts bidi streams on the HTTP proxy ALPN and forwards
/// to local Docker container ports.
pub async fn start_forward_handler(endpoint: Endpoint) -> Result<()> {
  info!("HTTP forward handler listening on ALPN {}", std::str::from_utf8(HTTP_PROXY_ALPN).unwrap_or("<binary>"));

  loop {
    let incoming = match endpoint.accept().await {
      Some(incoming) => incoming,
      None => {
        warn!("Endpoint accept returned None, forward handler exiting");
        break;
      }
    };

    let conn = match incoming.await {
      Ok(conn) => conn,
      Err(e) => {
        error!("Failed to accept connection: {e}");
        continue;
      }
    };

    // Verify ALPN
    let alpn = conn.alpn().to_vec();
    if alpn != HTTP_PROXY_ALPN {
      // Not our ALPN, skip (control plane handles luddite/control/1)
      continue;
    }

    tokio::spawn(async move {
      loop {
        match conn.accept_bi().await {
          Ok((mut send, mut recv)) => {
            tokio::spawn(async move {
              if let Err(e) = handle_stream(&mut send, &mut recv).await {
                error!("Stream handling error: {e:#}");
              }
            });
          }
          Err(e) => {
            // Connection closed
            debug!("Connection closed: {e}");
            break;
          }
        }
      }
    });
  }

  Ok(())
}

async fn handle_stream(
  send: &mut iroh::SendStream,
  recv: &mut iroh::RecvStream,
) -> anyhow::Result<()> {
  // Read target port (u16)
  let mut port_buf = [0u8; 2];
  recv.read_exact(&mut port_buf).await?;
  let target_port = u16::from_be_bytes(port_buf);

  // Read the rest of the stream (raw HTTP request)
  let mut request_buf = Vec::new();
  recv.read_to_end(&mut request_buf).await?;

  // Forward to localhost container port
  let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{target_port}"))
    .await
    .map_err(|e| anyhow::anyhow!("Failed to connect to container port {target_port}: {e}"))?;

  // Write request
  stream.write_all(&request_buf).await?;
  stream.shutdown().await?;

  // Read response
  let mut response_buf = Vec::new();
  stream.read_to_end(&mut response_buf).await?;

  // Send response back over Iroh stream
  send.write_all(&response_buf).await?;
  send.finish().await?;

  Ok(())
}
```

- [ ] **Step 4: Add module to Periphery main.rs**

In `bin/periphery/src/main.rs`, add:

```rust
mod http_bridge;
```

And in the startup section (inside the `async` block, after the outbound connection spawning), add:

```rust
    // Start HTTP forward handler (all nodes)
    {
      let endpoint = endpoint.clone();
      tokio::spawn(async move {
        if let Err(e) = http_bridge::forward::start_forward_handler(endpoint).await {
          error!("HTTP forward handler error: {e:#}");
        }
      });
    }

    // Start HTTP ingress bridge (ingress nodes only)
    if config.ingress_enabled {
      let endpoint = endpoint.clone();
      let port = config.http_bridge_port;
      tokio::spawn(async move {
        if let Err(e) = http_bridge::ingress::start_ingress_bridge(endpoint, port).await {
          error!("HTTP ingress bridge error: {e:#}");
        }
      });
    }
```

Note: `config.ingress_enabled` / `config.http_bridge_port` come from the `PeripheryConfig` additions in Task 2. However, `ingress_enabled` is on `ServerConfig`, not `PeripheryConfig`. The Periphery doesn't load `ServerConfig` — that's a Core entity.

**Design note:** The Periphery needs to know if it's an ingress node. Since `ServerConfig` is a Core entity, the Periphery should get this flag from its own config. Add `ingress_enabled: bool` to `PeripheryConfig` as well (the Periphery declares itself as ingress, and Core validates/updates the `ServerConfig` to match).

- [ ] **Step 5: Add ingress_enabled to PeripheryConfig (design fix)**

In `client/core/rs/src/entities/config/periphery.rs`, add to `PeripheryConfig`:

```rust
  /// Whether this node is an ingress node (runs Caddy + HTTP bridge listener).
  /// Default: false
  #[serde(default)]
  pub ingress_enabled: bool,
```

- [ ] **Step 6: Fix main.rs to use config.ingress_enabled**

Update the main.rs spawn block from Step 4 to use `config.ingress_enabled`.

- [ ] **Step 7: Add necessary Cargo dependencies**

In `bin/periphery/Cargo.toml`, ensure `axum`, `http-body-util` are available. Add if missing:

```toml
http-body-util = "0.1"
```

(already has `axum` via workspace? Check — if not, add `axum = { workspace = true }`)

- [ ] **Step 8: Build and verify**

Run:
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo check -p komodo_periphery 2>&1 | tail -20
```
Expected: 0 errors (warnings about unused code OK).

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(periphery): add Iroh HTTP bridge data plane

Ingress side: axum HTTP listener on 127.0.0.1:<port> that reads
X-Target-Endpoint + X-Target-Port headers, opens Iroh bidi stream
(ALPN luddite/http-proxy/1), pipes raw HTTP request/response.
Worker side: accept handler that reads target port prefix, forwards
to local Docker container port, pipes response back.
Connection pooling per worker endpoint ID."
```

---

## Task 6: Caddy JSON Config Builder (Core Side)

**Covers:** [S6]

**Files:**
- Create: `bin/core/src/ingress/mod.rs`
- Create: `bin/core/src/ingress/config.rs`

**Interfaces:**
- Consumes: `DeploymentInfo.host_ports`, `ServerConfig.ingress_enabled/public_ipv4`, `DnsProviderConfig`
- Produces: `build_caddy_json_config(routes, cloudflare_token)` → `serde_json::Value`

- [ ] **Step 1: Create the ingress module**

Create `bin/core/src/ingress/mod.rs`:

```rust
pub mod config;

pub use config::*;
```

Create `bin/core/src/ingress/config.rs`:

```rust
use serde::{Serialize, Deserialize};
use serde_json::{json, Value};

/// A single route entry for the Caddy config.
pub struct CaddyRoute {
  pub hostname: String,
  pub target_endpoint_id: String,
  pub target_port: u16,
}

/// Build the complete Caddy JSON config from a list of routes.
/// This is POSTed to Caddy's admin API at /load.
pub fn build_caddy_config(
  routes: &[CaddyRoute],
  cloudflare_api_token: &str,
  bridge_port: u16,
) -> Value {
  let route_entries: Vec<Value> = routes
    .iter()
    .map(|route| {
      json!({
        "match": [{
          "host": [route.hostname]
        }],
        "handle": [{
          "handler": "reverse_proxy",
          "upstreams": [{
            "dial": format!("127.0.0.1:{bridge_port}")
          }],
          "headers": {
            "request": {
              "set": {
                "X-Target-Endpoint": [route.target_endpoint_id],
                "X-Target-Port": [route.target_port.to_string()]
              }
            }
          }
        }]
      })
    })
    .collect();

  json!({
    "apps": {
      "http": {
        "servers": {
          "main": {
            "listen": [":80", ":443"],
            "automatic_https": {
              "disable_redirects": false
            },
            "routes": route_entries
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
                    "api_token": cloudflare_api_token
                  }
                }
              }
            }]
          }]
        }
      }
    }
  })
}
```

- [ ] **Step 2: Add module to main.rs**

In `bin/core/src/main.rs`, add:

```rust
mod ingress;
```

- [ ] **Step 3: Build and verify**

Run:
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo check -p komodo_core 2>&1 | tail -10
```
Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(core): add Caddy JSON config builder

serde_json::Value builder for Caddy admin API. Builds reverse_proxy
routes with X-Target-Endpoint/X-Target-Port headers, TLS automation
with cloudflare DNS-01 challenge. POSTed to Caddy /load endpoint."
```

---

## Task 7: Caddy Supervisor (Periphery Side)

**Covers:** [S8]

**Files:**
- Create: `bin/periphery/src/caddy/mod.rs`
- Create: `bin/periphery/src/caddy/supervisor.rs`
- Create: `bin/periphery/src/caddy/binary.rs`
- Modify: `bin/periphery/src/main.rs` (start Caddy supervisor if ingress_enabled)

**Interfaces:**
- Consumes: `PeripheryConfig.caddy_binary_path`, `PeripheryConfig.vendored_manifest_url`
- Produces: `start_caddy_supervisor(config)`, `reload_caddy_config(json)` admin API client

- [ ] **Step 1: Create the binary management module**

Create `bin/periphery/src/caddy/binary.rs`:

```rust
use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::{info, warn};

#[derive(Debug, Deserialize)]
struct Manifest {
  artifacts: std::collections::HashMap<String, Artifact>,
}

#[derive(Debug, Deserialize)]
struct Artifact {
  version: String,
  checksums: std::collections::HashMap<String, String>,
  download_url: String,
}

/// Check if the local Caddy binary matches the latest version in manifest.
/// Downloads and swaps if newer.
pub async fn ensure_caddy_binary(
  binary_path: &str,
  manifest_url: &str,
) -> Result<()> {
  let manifest = fetch_manifest(manifest_url).await?;
  let caddy = manifest
    .artifacts
    .get("caddy")
    .context("No caddy artifact in manifest")?;

  // Check current version
  let current_version = get_local_caddy_version(binary_path).await;

  if current_version.as_deref() == Some(caddy.version.as_str()) {
    info!("Caddy binary up to date (v{})", caddy.version);
    return Ok(());
  }

  info!(
    "Updating Caddy binary from {:?} to v{}",
    current_version, caddy.version
  );

  // Determine arch
  let arch = if cfg!(target_arch = "x86_64") {
    "linux-amd64"
  } else if cfg!(target_arch = "aarch64") {
    "linux-arm64"
  } else {
    anyhow::bail!("Unsupported architecture for Caddy binary download")
  };

  let checksum = caddy
    .checksums
    .get(arch)
    .context(format!("No checksum for {arch}"))?;

  // Download URL with arch substituted
  let url = caddy.download_url.replace("{{arch}}", arch);

  // Download
  let resp = reqwest::get(&url)
    .await
    .context("Failed to download Caddy binary")?;
  let bytes = resp
    .bytes()
    .await
    .context("Failed to read Caddy binary response")?;

  // Verify checksum
  let computed = sha256(&bytes);
  let expected = checksum.strip_prefix("sha256:").unwrap_or(checksum);
  if computed != expected {
    anyhow::bail!(
      "Checksum mismatch: expected {expected}, got {computed}"
    );
  }

  // Write to temp then atomic swap
  let tmp_path = format!("{binary_path}.tmp");
  if let Some(parent) = std::path::Path::new(&tmp_path).parent() {
    std::fs::create_dir_all(parent).ok();
  }
  std::fs::write(&tmp_path, &bytes)?;
  std::fs::rename(&tmp_path, binary_path)?;

  // Make executable
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(binary_path, perms)?;
  }

  info!("Caddy binary updated to v{}", caddy.version);
  Ok(())
}

async fn fetch_manifest(url: &str) -> Result<Manifest> {
  let resp = reqwest::get(url)
    .await
    .context("Failed to fetch vendored manifest")?;
  let manifest: Manifest =
    resp.json().await.context("Failed to parse manifest")?;
  Ok(manifest)
}

async fn get_local_caddy_version(binary_path: &str) -> Option<String> {
  if !std::path::Path::new(binary_path).exists() {
    return None;
  }
  let output = tokio::process::Command::new(binary_path)
    .arg("version")
    .output()
    .await
    .ok()?;
  let stdout = String::from_utf8_lossy(&output.stdout);
  // Parse "v2.9.1" from "v2.9.1 ..." output
  let version = stdout
    .split_whitespace()
    .next()?
    .trim_start_matches('v')
    .to_string();
  Some(version)
}

fn sha256(data: &[u8]) -> String {
  use sha2::{Digest, Sha256};
  let mut hasher = Sha256::new();
  hasher.update(data);
  hex::encode(hasher.finalize())
}
```

- [ ] **Step 2: Create the supervisor module**

Create `bin/periphery/src/caddy/supervisor.rs`:

```rust
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::process::Command;
use tracing::{error, info};

/// Start the Caddy process as a child, supervised by Periphery.
/// Caddy's admin API runs on 127.0.0.1:2019.
pub async fn start_caddy(binary_path: &str) -> Result<()> {
  info!("Starting Caddy process: {binary_path}");

  let mut child = Command::new(binary_path)
    .arg("run")
    .arg("--adapter")
    .arg("json")
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .context("Failed to spawn Caddy process")?;

  // Wait for Caddy to start (admin API becomes available)
  tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

  // Check if process is still alive
  match child.try_wait() {
    Ok(Some(status)) => {
      anyhow::bail!("Caddy exited immediately with status: {status}");
    }
    Ok(None) => {
      info!("Caddy process started successfully");
    }
    Err(e) => {
      anyhow::bail!("Failed to check Caddy process status: {e}");
    }
  }

  // Spawn a task to wait for Caddy process exit
  tokio::spawn(async move {
    let status = child.wait().await;
    match status {
      Ok(s) => error!("Caddy process exited: {s}"),
      Err(e) => error!("Caddy process wait error: {e}"),
    }
  });

  Ok(())
}

/// Push a new JSON config to Caddy's admin API.
/// POST /load with Content-Type: application/json.
/// Hot reload — zero downtime.
pub async fn reload_config(
  config: &serde_json::Value,
) -> Result<()> {
  let client = reqwest::Client::new();
  let resp = client
    .post("http://127.0.0.1:2019/load")
    .header("Content-Type", "application/json")
    .json(config)
    .send()
    .await
    .context("Failed to POST config to Caddy admin API")?;

  if !resp.status().is_success() {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    anyhow::bail!("Caddy config reload failed ({status}): {body}");
  }

  info!("Caddy config reloaded successfully");
  Ok(())
}
```

- [ ] **Step 3: Create the caddy module mod.rs**

Create `bin/periphery/src/caddy/mod.rs`:

```rust
pub mod binary;
pub mod supervisor;
```

- [ ] **Step 4: Add module + dependencies**

In `bin/periphery/src/main.rs`, add:

```rust
mod caddy;
```

In the startup section (if `config.ingress_enabled`), add:

```rust
    // Start Caddy (ingress nodes only)
    if config.ingress_enabled {
      let binary_path = config.caddy_binary_path.clone();
      let manifest_url = config.vendored_manifest_url.clone();

      tokio::spawn(async move {
        // Ensure binary is downloaded and up to date
        if let Err(e) = caddy::binary::ensure_caddy_binary(&binary_path, &manifest_url).await {
          error!("Failed to ensure Caddy binary: {e:#}");
          return;
        }

        // Start Caddy process
        if let Err(e) = caddy::supervisor::start_caddy(&binary_path).await {
          error!("Failed to start Caddy: {e:#}");
          return;
        }

        info!("Caddy supervisor running");
      });
    }
```

In `bin/periphery/Cargo.toml`, add:

```toml
sha2 = "0.10"
hex = "0.4"
```

(`reqwest` and `tokio` should already be available from workspace deps)

- [ ] **Step 5: Build and verify**

Run:
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo check -p komodo_periphery 2>&1 | tail -20
```
Expected: 0 errors.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(periphery): add Caddy supervisor + binary management

Binary management: fetches manifest.json from luddite-dev/vendored,
downloads Caddy binary if version mismatch, verifies SHA256 checksum,
atomic swap. Supervisor: spawns Caddy as child process, admin API
client for hot config reload via POST /load with JSON."
```

---

## Task 8: DNS Record Management + Failover

**Covers:** [S4, S5]

**Files:**
- Create: `bin/core/src/ingress/management.rs` (deployment → DNS → Caddy config orchestration)
- Create: `bin/core/src/ingress/failover.rs` (ingress node failure handling)
- Modify: `bin/core/src/ingress/mod.rs` (add submodules)

**Interfaces:**
- Consumes: `DnsProvider` trait (Task 1), `build_caddy_config` (Task 6), `DnsRecord` entity (Task 1)
- Produces: `create_app_dns_record()`, `delete_app_dns_record()`, `migrate_ingress_records()`, `get_ingress_routes_for_node()`

- [ ] **Step 1: Create the management module**

Create `bin/core/src/ingress/management.rs`:

```rust
use anyhow::Result;
use tracing::{info, warn};

use crate::dns::provider::DnsProvider;
use crate::entities::dns::{DnsRecord, DnsRecordType};

/// Create DNS A record for an app and store in DB.
pub async fn create_app_dns_record(
  provider: &dyn DnsProvider,
  base_domain: &str,
  subdomain: &str,
  ingress_node_id: &str,
  ingress_node_ip: &str,
  deployment_id: &str,
) -> Result<DnsRecord> {
  let hostname = format!("{subdomain}.{base_domain}");
  let zone_id = provider.resolve_zone_id(base_domain).await?;
  let record_id = provider
    .create_record(
      &zone_id,
      DnsRecordType::A,
      &hostname,
      ingress_node_ip,
      60,
    )
    .await?;

  info!("Created DNS A record: {hostname} → {ingress_node_ip}");

  let record = DnsRecord {
    id: uuid::Uuid::new_v4().to_string(),
    record_type: DnsRecordType::A,
    hostname,
    target_node_id: ingress_node_id.to_string(),
    provider_type: "cloudflare".to_string(),
    provider_zone_id: zone_id,
    provider_record_id: record_id,
    deployment_id: Some(deployment_id.to_string()),
    ttl: 60,
    created_at: chrono::Utc::now(),
    updated_at: chrono::Utc::now(),
  };

  // TODO: Save to MongoDB (dns_records collection)
  // db.collection("dns_records").insert_one(&record, None).await?;

  Ok(record)
}

/// Delete DNS record for an app.
pub async fn delete_app_dns_record(
  provider: &dyn DnsProvider,
  record: &DnsRecord,
) -> Result<()> {
  provider
    .delete_record(&record.provider_zone_id, &record.provider_record_id)
    .await?;

  info!("Deleted DNS record: {}", record.hostname);

  // TODO: Remove from MongoDB (dns_records collection)
  // db.collection("dns_records").delete_one(doc! { "id": &record.id }, None).await?;

  Ok(())
}

/// Update a DNS record's target IP (for failover).
pub async fn update_record_target(
  provider: &dyn DnsProvider,
  record: &DnsRecord,
  new_ip: &str,
) -> Result<()> {
  provider
    .update_record(&record.provider_zone_id, &record.provider_record_id, new_ip)
    .await?;

  info!(
    "Updated DNS record {}: → {new_ip}",
    record.hostname
  );

  // TODO: Update MongoDB record

  Ok(())
}
```

- [ ] **Step 2: Create the failover module**

Create `bin/core/src/ingress/failover.rs`:

```rust
use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

use crate::dns::provider::DnsProvider;
use crate::entities::dns::DnsRecord;
use super::management::update_record_target;

/// Migrate all DNS records from a failed ingress node to a new one.
/// Called when an ingress node transitions to ServerState::NotOk.
pub async fn migrate_ingress_records(
  provider: &dyn DnsProvider,
  failed_node_id: &str,
  new_node_id: &str,
  new_node_ip: &str,
) -> Result<Vec<DnsRecord>> {
  // TODO: Query MongoDB for all DnsRecord where target_node_id = failed_node_id
  let records: Vec<DnsRecord> = vec![]; // Placeholder — implement with actual DB

  let mut migrated = Vec::new();

  for record in &records {
    match update_record_target(provider, record, new_node_ip).await {
      Ok(()) => {
        info!(
          "Migrated {} from {} to {}",
          record.hostname, failed_node_id, new_node_id
        );
        migrated.push(record.clone());
      }
      Err(e) => {
        warn!(
          "Failed to migrate {}: {e:#}",
          record.hostname
        );
      }
    }
  }

  // TODO: Update all migrated records in DB: target_node_id = new_node_id

  info!(
    "Migrated {}/{} DNS records from {} to {}",
    migrated.len(),
    records.len(),
    failed_node_id,
    new_node_id
  );

  Ok(migrated)
}
```

- [ ] **Step 3: Update ingress mod.rs**

Update `bin/core/src/ingress/mod.rs`:

```rust
pub mod config;
pub mod management;
pub mod failover;

pub use config::*;
```

- [ ] **Step 4: Build and verify**

Run:
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo check -p komodo_core 2>&1 | tail -10
```
Expected: 0 errors.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): add DNS record management + ingress failover

Management: create/delete/update app DNS A records via DnsProvider trait.
Failover: migrate all DNS records from a failed ingress node to a new
one — updates Cloudflare records, changes target_node_id in DB.
DB integration marked as TODO — needs MongoDB collection wiring."
```

---

## Task 9: Vendored Repo Setup

**Covers:** [S8]

**Files:**
- Create: (in separate repo `luddite-dev/vendored`) `.github/workflows/caddy-check.yml`, `.github/workflows/caddy-build.yml`, `manifest.json`, `README.md`

**Interfaces:**
- Produces: `manifest.json` with Caddy version + checksums + download URLs

- [ ] **Step 1: Create the vendored repo structure locally**

Create a new local directory (not inside the deploy repo) and initialize the vendored repo:

```bash
mkdir -p /tmp/vendored
cd /tmp/vendored
git init
gh repo create luddite-dev/vendored --public --source=. --push
```

- [ ] **Step 2: Create manifest.json**

```json
{
  "version": 1,
  "artifacts": {
    "caddy": {
      "version": "0.0.0",
      "upstream_version": "",
      "plugins": [
        {
          "name": "cloudflare-dns",
          "module": "github.com/caddy-dns/cloudflare",
          "version": ""
        }
      ],
      "checksums": {},
      "download_url": ""
    }
  }
}
```

- [ ] **Step 3: Create caddy-check.yml workflow**

Create `.github/workflows/caddy-check.yml`:

```yaml
name: Check Caddy Updates

on:
  schedule:
    # Daily at 3:17 AM UTC (avoid :00 crunch)
    - cron: '17 3 * * *'
  workflow_dispatch:

jobs:
  check:
    runs-on: ubuntu-latest
    outputs:
      latest_version: ${{ steps.check.outputs.latest_version }}
      needs_update: ${{ steps.check.outputs.needs_update }}
    steps:
      - uses: actions/checkout@v4

      - name: Get latest Caddy release
        id: check
        run: |
          LATEST=$(gh release view --repo caddyserver/caddy --json tagName -q .tagName | sed 's/^v//')
          CURRENT=$(jq -r '.artifacts.caddy.version' manifest.json)
          
          echo "latest_version=$LATEST" >> $GITHUB_OUTPUT
          echo "Latest Caddy: $LATEST, Current: $CURRENT"
          
          if [ "$LATEST" != "$CURRENT" ]; then
            echo "needs_update=true" >> $GITHUB_OUTPUT
          else
            echo "needs_update=false" >> $GITHUB_OUTPUT
          fi
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}

      - name: Trigger build
        if: steps.check.outputs.needs_update == 'true'
        run: |
          gh workflow run caddy-build.yml \
            -f caddy_version=${{ steps.check.outputs.latest_version }}
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

- [ ] **Step 4: Create caddy-build.yml workflow**

Create `.github/workflows/caddy-build.yml`:

```yaml
name: Build Caddy Binary

on:
  workflow_dispatch:
    inputs:
      caddy_version:
        description: 'Caddy version (without v prefix)'
        required: true

jobs:
  build:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        include:
          - goarch: amd64
            target: linux-amd64
          - goarch: arm64
            target: linux-arm64
    steps:
      - uses: actions/checkout@v4

      - uses: actions/setup-go@v5
        with:
          go-version: '1.23'

      - name: Install xcaddy
        run: go install github.com/caddyserver/xcaddy/cmd/xcaddy@latest

      - name: Build Caddy
        run: |
          xcaddy build v${{ inputs.caddy_version }} \
            --with github.com/caddy-dns/cloudflare \
            --output caddy-luddite-${{ inputs.caddy_version }}-${{ matrix.target }}
        env:
          GOARCH: ${{ matrix.goarch }}
          CGO_ENABLED: '0'

      - name: Compute checksum
        id: checksum
        run: |
          sha256sum caddy-luddite-* > checksums.txt
          cat checksums.txt

      - name: Upload release asset
        run: |
          gh release create caddy-${{ inputs.caddy_version }} \
            caddy-luddite-${{ inputs.caddy_version }}-${{ matrix.target }} \
            --title "Caddy ${{ inputs.caddy_version }}" \
            --notes "Vendored Caddy build v${{ inputs.caddy_version }} with caddy-dns/cloudflare plugin"
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}

      - name: Update manifest
        run: |
          AMD64_CHECKSUM=$(sha256sum caddy-luddite-*-linux-amd64 | cut -d' ' -f1)
          ARM64_CHECKSUM=$(sha256sum caddy-luddite-*-linux-arm64 | cut -d' ' -f1)
          
          jq \
            --arg version "${{ inputs.caddy_version }}" \
            --arg upstream "v${{ inputs.caddy_version }}" \
            --arg amd64 "sha256:$AMD64_CHECKSUM" \
            --arg arm64 "sha256:$ARM64_CHECKSUM" \
            --arg url "https://github.com/luddite-dev/vendored/releases/download/caddy-${{ inputs.caddy_version }}/caddy-luddite-${{ inputs.caddy_version }}-linux-{{arch}}" \
            '.artifacts.caddy.version = $version | .artifacts.caddy.upstream_version = $upstream | .artifacts.caddy.checksums = {"linux-amd64": $amd64, "linux-arm64": $arm64} | .artifacts.caddy.download_url = $url' \
            manifest.json > manifest.json.tmp
          mv manifest.json.tmp manifest.json

      - name: Commit manifest
        run: |
          git config user.name "github-actions[bot]"
          git config user.email "github-actions[bot]@users.noreply.github.com"
          git add manifest.json
          git commit -m "chore: update caddy to v${{ inputs.caddy_version }}"
          git push
```

- [ ] **Step 5: Create README.md**

```markdown
# Vendored Binaries

This repository manages the build pipeline for vendored binaries used by Luddite.

## Artifacts

### Caddy

Custom Caddy build with the `caddy-dns/cloudflare` plugin baked in.

- **Manifest:** `manifest.json` tracks the latest version, checksums, and download URLs.
- **CI:** Daily check for new upstream Caddy releases. Builds and publishes automatically.
- **Consumers:** Periphery fetches `manifest.json` to detect version changes and auto-updates the local binary.
```

- [ ] **Step 6: Commit and push the vendored repo**

```bash
cd /tmp/vendored
git add -A
git commit -m "init: vendored binary pipeline — Caddy with cloudflare DNS plugin"
git push origin main
```

- [ ] **Step 7: Trigger first build manually**

```bash
gh workflow run caddy-build.yml -f caddy_version=2.9.1
```

Wait for the build to complete and verify `manifest.json` gets updated.

---

## Task 10: Wire Deployment Lifecycle to DNS + Caddy

**Covers:** [S10]

**Files:**
- Modify: `bin/core/src/resource/deployment.rs` (on create: create DNS record, push Caddy config; on delete: delete DNS record, push Caddy config)
- Modify: `bin/core/src/ingress/management.rs` (wire to real MongoDB, remove TODOs)

**Interfaces:**
- Consumes: All prior tasks (DnsProvider, Caddy config builder, management module)
- Produces: End-to-end flow — deploy app → DNS created → Caddy configured → traffic flows

- [ ] **Step 1: Wire management module to MongoDB**

In `bin/core/src/ingress/management.rs`, replace the `// TODO: Save to MongoDB` comments with actual database calls. Follow the existing pattern in `bin/core/src/resource/` for how MongoDB collections are accessed (look at `bin/core/src/db.rs` or equivalent).

The `dns_records` collection stores `DnsRecord` documents. Implement:
- `insert_dns_record(record)` 
- `delete_dns_record(id)` 
- `find_dns_records_by_node(node_id)` 
- `find_dns_record_by_deployment(deployment_id)` 
- `update_dns_record_target(id, new_node_id)`

- [ ] **Step 2: Hook into deployment create**

In `bin/core/src/resource/deployment.rs`, in the `post_create` function (around line 200-214), after `info.assigned_server` is set and `ReadContainerPorts` readback (Task 4) populates `info.host_ports`:

```rust
// If http_proxy is configured, create DNS record + push Caddy config
if let Some(http_proxy) = &config.http_proxy {
  if let Some(dns_provider) = crate::dns::build_dns_provider()? {
    let base_domain = &core_config().ingress.dns.base_domain;
    if let Some(base_domain) = base_domain {
      // Find or select an ingress node
      let ingress_node = select_ingress_node().await?;
      
      let record = crate::ingress::management::create_app_dns_record(
        dns_provider.as_ref(),
        base_domain,
        &http_proxy.subdomain,
        &ingress_node.id,
        &ingress_node.public_ipv4,
        &deployment.id,
      ).await?;

      // Push updated Caddy config to the ingress node
      let routes = get_ingress_routes_for_node(&ingress_node.id).await?;
      let caddy_config = crate::ingress::config::build_caddy_config(
        &routes,
        &cloudflare_api_token,
        ingress_node.http_bridge_port,
      );
      
      // Send config to Periphery via Iroh control plane
      send_caddy_config_to_periphery(&ingress_node.id, &caddy_config).await?;
    }
  }
}
```

(Actual function signatures may vary — follow existing code patterns for how Core communicates with Periphery via the Iroh control plane.)

- [ ] **Step 3: Hook into deployment delete**

In the deployment delete path, before the deployment is removed:

```rust
// Delete DNS record if http_proxy was configured
// Find and delete the DnsRecord for this deployment
```

- [ ] **Step 4: Build and verify**

Run:
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo check --workspace 2>&1 | tail -20
```
Expected: 0 errors.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: wire deployment lifecycle to DNS + Caddy config

On create with http_proxy: create Cloudflare A record, store in
dns_records DB, push Caddy JSON config to ingress Periphery.
On delete: delete DNS record, remove from DB, push updated Caddy config.
End-to-end: deploy app → DNS → cert → Caddy → Iroh bridge → container."
```

---

## Task 11: fmt, clippy, final verification + docs

**Covers:** [S11, S12]

**Files:**
- All files touched in prior tasks
- Modify: `roadmap.md` (mark M4 section)
- Modify: `readme.md` (add M4 section)

- [ ] **Step 1: Format**

```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo fmt
```

- [ ] **Step 2: Clippy**

```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo clippy --workspace 2>&1 | tail -30
```
Fix any warnings.

- [ ] **Step 3: Full workspace check**

```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo check --workspace 2>&1 | tail -10
```
Expected: 0 errors, 0 warnings.

- [ ] **Step 4: Run existing tests**

```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo test -p transport --lib iroh 2>&1 | tail -10
```
Expected: 5/5 pass (existing M3 tests).

- [ ] **Step 5: Update roadmap.md**

Read `roadmap.md` and mark M4 as complete (or in progress, depending on actual completion status).

- [ ] **Step 6: Update docs/forking.md**

Add notes about the new DNS/Caddy config fields and the vendored repo.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "chore: fmt, clippy fixes, docs for M4 Caddy+DNS ingress"
```

- [ ] **Step 8: Push and create draft PR**

```bash
git push -u origin caddy-dns-ingress
gh pr create --draft --base main --title "M4: Caddy + DNS Ingress" --body "## Summary
- Auto HTTPS ingress for user-deployed Docker web apps
- Caddy reverse proxy on dedicated ingress nodes (JSON config, admin API hot reload)
- Cloudflare DNS management (trait-abstracted DnsProvider, first impl)
- Iroh HTTP bridge data plane (ALPN luddite/http-proxy/1)
- Vendored binary pipeline via luddite-dev/vendored repo
- Ingress node failover with DNS record migration
- ReadContainerPorts readback (resolves TODO Task 8)"
```
