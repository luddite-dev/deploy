# Stack HTTP Proxy / Ingress Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use compose:subagent (recommended) or compose:execute to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add automatic DNS + Caddy reverse proxy + HTTPS endpoint allocation to Stack (compose) resources, mirroring the existing Deployment ingress flow.

**Architecture:** A single optional `http_proxy: Option<StackHttpProxyConfig>` on `StackConfig` selects one compose service + subdomain + container port. After `DeployStack::resolve` runs `ComposeUp` (which returns service container names), we read back the proxied service's host ports, then create a DNS record + push a Caddy route to the ingress node. Delete/update tear down + rebuild. `build_ingress_routes` is extended to also query stacks, so every Caddy rebuild includes both deployment and stack routes.

**Tech Stack:** Rust (komodo_core bin, komodo_client entities), TypeScript/React (Mantine UI), typeshare for RS→TS type propagation, MongoDB (mungos), Iroh + Caddy (periphery ingress).

## Global Constraints

- **Build:** Always set `CARGO_TARGET_DIR=/home/acheong/.cargo-target` when running `cargo build`/`cargo test` in a worktree (shared target dir convention).
- **UI deps:** `ui/node_modules` is a symlink to the main checkout's `node_modules` — do NOT run `npm install`; it fails due to peer-dep conflicts.
- **typeshare:** Changes to `client/core/rs/src/entities/*` propagate to `client/core/ts/src/types.ts` + `ui/public/client/types.d.ts` via the build step (`cargo build` runs typeshare). Use the generated `Types.*` in UI code.
- **Best-effort ingress:** Failures in `try_setup_*_ingress` / `try_teardown_*_ingress` are logged at `warn!` and never fail a deploy/delete (matches deployment pattern at `deployment.rs:243-248`).
- **Server mode only:** Stack ingress is for Server-mode stacks, not Swarm. The ingress layer assumes Iroh endpoint routing.
- **Pre-existing WIP:** Files with user's unrelated swarm-removal WIP exist in the main checkout but NOT in this worktree (fresh from `cb5ab74ca`). Do not pull them in.
- **Spec:** `docs/compose/specs/2026-07-23-stack-ingress-design.md` — sections [S1]–[S9].

---

### Task 1: Add `StackHttpProxyConfig` entity + `stack_id` on `DnsRecord`

**Covers:** S1, S3

**Files:**
- Modify: `client/core/rs/src/entities/stack.rs` (add struct + field on StackConfig)
- Modify: `client/core/rs/src/entities/dns.rs:33` (add `stack_id` field on DnsRecord)

**Interfaces:**
- Produces: `StackHttpProxyConfig { service: String, subdomain: String, container_port: u16 }` type; `StackConfig.http_proxy: Option<StackHttpProxyConfig>` field; `DnsRecord.stack_id: Option<String>` field. All typeshare'd so they appear in `Types.*` TS client after build.

- [ ] **Step 1: Add the struct to stack.rs**

In `client/core/rs/src/entities/stack.rs`, add the new struct near the bottom of the file (after `StackService` or alongside other typeshare structs). Place it after the `AssignedPort` re-use — it lives in this entities module:

```rust
#[typeshare]
#[derive(
  Debug, Clone, Default, PartialEq, Serialize, Deserialize,
)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct StackHttpProxyConfig {
  /// Which compose service to proxy to. Must match a service name
  /// declared in the compose file.
  pub service: String,
  /// Subdomain. FQDN = "{subdomain}.{ingress.dns.base_domain}".
  pub subdomain: String,
  /// Which container port on that service receives proxied traffic.
  pub container_port: u16,
}
```

- [ ] **Step 2: Add the field to StackConfig**

In `client/core/rs/src/entities/stack.rs`, find `pub struct StackConfig` (line ~322). Add the field alongside the other `#[serde(default)]` fields, following the same attribute pattern as `DeploymentConfig.http_proxy` (`deployment.rs:266-271`). Add it near the end of the struct, before the closing brace. Also add it to the `Default` impl — but `StackConfig` uses `#[derive(Default)]` via the Builder/Partial macro, so explicit Default impl may not exist; check. If there's no manual `impl Default`, the derive handles `Option::default()` = `None` automatically.

```rust
  /// HTTP ingress configuration for this stack.
  /// When set, creates a DNS record + Caddy route for automatic HTTPS,
  /// pointing at one service's container port.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  #[partial_attr(serde(default))]
  #[builder(default)]
  pub http_proxy: Option<StackHttpProxyConfig>,
```

- [ ] **Step 3: Export the type from the entities mod**

In `client/core/rs/src/entities/mod.rs`, check if `StackHttpProxyConfig` needs to be re-exported. Look at how `StackConfig` is exported. Add `StackHttpProxyConfig` to the `pub use` for the `stack` module if `StackConfig` is re-exported there (grep for `StackConfig` in mod.rs).

- [ ] **Step 4: Add `stack_id` to DnsRecord**

In `client/core/rs/src/entities/dns.rs`, in `pub struct DnsRecord` (line 33), add after the `deployment_id` field (line 49-50):

```rust
  /// The stack this record is attached to, if any.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub stack_id: Option<String>,
```

- [ ] **Step 5: Build to regenerate typeshare + verify compilation**

Run:
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo build 2>&1 | tail -8
```
Expected: `Finished dev profile` with no new errors (pre-existing 8 warnings OK).

- [ ] **Step 6: Verify types propagated to TS**

Run:
```bash
grep -n "StackHttpProxyConfig\|stack_id" ui/public/client/types.d.ts | head -10
```
Expected: `StackHttpProxyConfig` interface + `stack_id?: string` on `DnsRecord` present.

- [ ] **Step 7: Commit**

```bash
git add client/core/rs/src/entities/stack.rs client/core/rs/src/entities/dns.rs client/core/rs/src/entities/mod.rs client/core/ts/src/types.ts ui/public/client/types.d.ts ui/public/client/types.js
git commit -m "feat(entities): add StackHttpProxyConfig + stack_id on DnsRecord

Adds the data model for per-stack HTTP proxy ingress: a single
optional http_proxy on StackConfig selecting one service + subdomain
+ container port. DnsRecord gains a parallel stack_id field (additive,
non-breaking) so stack DNS records don't overload deployment_id."
```

---

### Task 2: Add stack DNS record lifecycle functions

**Covers:** S3, S5

**Files:**
- Modify: `bin/core/src/ingress/management.rs` (add `create_stack_dns_record` + `delete_stack_dns_records`)

**Interfaces:**
- Consumes: `DnsRecord`, `DnsRecordType`, `IngressConfig` from `komodo_client::entities::dns`; `build_dns_provider` from `crate::dns`; `db_client` from `crate::state`.
- Produces: `create_stack_dns_record(stack_id, hostname, target_node_id, target_ipv4, target_ipv6, ingress_config, ttl) -> Result<()>` and `delete_stack_dns_records(stack_id, ingress_config) -> Result<()>`. These are the stack parallels of the existing `create_deployment_dns_record` (line 57) / `delete_deployment_dns_records` (line 144).

- [ ] **Step 1: Add `create_stack_dns_record`**

In `bin/core/src/ingress/management.rs`, after `create_deployment_dns_record` (ends line 137), add a parallel function. It is identical except it persists `stack_id: Some(...)` and `deployment_id: None`:

```rust
/// Create DNS A/AAAA records for a stack hostname and persist
/// each to the database. Parallel to `create_deployment_dns_record`
/// but keyed by `stack_id`.
#[allow(clippy::too_many_arguments)]
pub async fn create_stack_dns_record(
  stack_id: &str,
  hostname: &str,
  target_node_id: &str,
  target_ipv4: Option<&str>,
  target_ipv6: Option<&str>,
  ingress_config: &IngressConfig,
  ttl: u32,
) -> Result<()> {
  let provider = ingress_provider()?;
  let base_domain = ingress_config
    .dns
    .base_domain
    .as_deref()
    .filter(|d| !d.is_empty())
    .ok_or_else(|| {
      anyhow::anyhow!(
        "ingress.dns.base_domain not configured — cannot build FQDN"
      )
    })?;
  let fqdn = format!("{hostname}.{base_domain}");
  let zone_id = provider
    .resolve_zone_id(base_domain)
    .await
    .with_context(|| format!("resolve zone id for {base_domain}"))?;
  let now = komodo_timestamp();

  if let Some(ipv4) = target_ipv4 {
    let record_id = provider
      .create_record(&zone_id, DnsRecordType::A, &fqdn, ipv4, ttl)
      .await
      .with_context(|| format!("create A record for {fqdn}"))?;
    let record = DnsRecord {
      id: ObjectId::new().to_string(),
      deployment_id: None,
      stack_id: Some(stack_id.to_string()),
      hostname: fqdn.clone(),
      record_type: DnsRecordType::A,
      target_node_id: target_node_id.to_string(),
      provider_type: ingress_config.dns.provider.clone(),
      provider_zone_id: zone_id.clone(),
      provider_record_id: record_id,
      ttl,
      created_at: now,
      updated_at: now,
    };
    dns_records()
      .insert_one(record)
      .await
      .with_context(|| format!("persist A record for {fqdn}"))?;
    info!("Created DNS A record for {fqdn} -> {ipv4}");
  }

  if let Some(ipv6) = target_ipv6 {
    let record_id = provider
      .create_record(&zone_id, DnsRecordType::AAAA, &fqdn, ipv6, ttl)
      .await
      .with_context(|| format!("create AAAA record for {fqdn}"))?;
    let record = DnsRecord {
      id: ObjectId::new().to_string(),
      deployment_id: None,
      stack_id: Some(stack_id.to_string()),
      hostname: fqdn.clone(),
      record_type: DnsRecordType::AAAA,
      target_node_id: target_node_id.to_string(),
      provider_type: ingress_config.dns.provider.clone(),
      provider_zone_id: zone_id.clone(),
      provider_record_id: record_id,
      ttl,
      created_at: now,
      updated_at: now,
    };
    dns_records()
      .insert_one(record)
      .await
      .with_context(|| format!("persist AAAA record for {fqdn}"))?;
    info!("Created DNS AAAA record for {fqdn} -> {ipv6}");
  }

  Ok(())
}
```

- [ ] **Step 2: Add `delete_stack_dns_records`**

After `delete_deployment_dns_records` (ends line 182), add:

```rust
/// Delete all DNS records for a stack (cleanup on stack delete).
/// Best-effort at the provider, same as `delete_deployment_dns_records`.
pub async fn delete_stack_dns_records(
  stack_id: &str,
  _ingress_config: &IngressConfig,
) -> Result<()> {
  let provider = ingress_provider()?;
  let records: Vec<DnsRecord> = find_collect(
    &dns_records(),
    doc! { "stack_id": stack_id },
    None,
  )
  .await
  .context("failed to query dns_records for stack")?;

  for record in records {
    if let Err(e) = provider
      .delete_record(
        &record.provider_zone_id,
        &record.provider_record_id,
      )
      .await
    {
      warn!(
        "Failed to delete DNS record {} ({}) at provider: {e:#}",
        record.hostname, record.provider_record_id
      );
    } else {
      info!(
        "Deleted DNS record {} for stack {stack_id}",
        record.hostname
      );
    }
  }

  dns_records()
    .delete_many(doc! { "stack_id": stack_id })
    .await
    .context("failed to delete dns_records from database")?;
  Ok(())
}
```

- [ ] **Step 3: Build to verify**

```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo build 2>&1 | tail -8
```
Expected: `Finished dev profile`. The new functions are `pub` but not yet called, so no dead-code error.

- [ ] **Step 4: Commit**

```bash
git add bin/core/src/ingress/management.rs
git commit -m "feat(ingress): add stack DNS record lifecycle functions

create_stack_dns_record / delete_stack_dns_records parallel the
deployment versions but persist DnsRecord.stack_id instead of
deployment_id. Used by the stack ingress setup/teardown paths."
```

---

### Task 3: Add `build_ingress_routes` stack integration

**Covers:** S6

**Files:**
- Modify: `bin/core/src/resource/deployment.rs:720` (extend `build_ingress_routes` to also query stacks)

**Interfaces:**
- Consumes: `Stack`, `StackHttpProxyConfig`, `StackServiceNames` from `komodo_client::entities::stack`; `db_client().stacks` collection; `get_server_for_command`.
- Produces: `build_ingress_routes` now returns routes for both deployments and stacks. Existing callers (`try_setup_ingress`, `try_teardown_ingress`) need no changes — they call this function which returns a combined `Vec<CaddyRoute>`.

- [ ] **Step 1: Add stack route collection to `build_ingress_routes`**

In `bin/core/src/resource/deployment.rs`, in `build_ingress_routes` (line 720), after the `for dep in deployments { ... }` loop (ends line 773), before `Ok(routes)`, add a stack loop:

```rust
  // Query all stacks that have http_proxy set.
  use komodo_client::entities::stack::Stack;
  let stacks: Vec<Stack> = find_collect(
    &db_client().stacks,
    doc! { "config.http_proxy": { "$ne": null } },
    None,
  )
  .await
  .context("failed to query stacks with http_proxy")?;

  for stack in stacks {
    let Some(http_proxy) = &stack.config.http_proxy else {
      continue;
    };
    let server =
      get_server_for_command(&stack.config.server_id).await.ok();
    let Some(server) = server else {
      warn!(
        "build_ingress_routes: could not get server for stack {} (server_id={}), skipping",
        stack.name, stack.config.server_id
      );
      continue;
    };
    let endpoint_id = server.info.endpoint_id.clone();
    if endpoint_id.is_empty() {
      continue;
    }
    // Find the host port for the proxied service + container port.
    let host_port = stack
      .info
      .host_ports
      .get(&http_proxy.service)
      .and_then(|ports| {
        ports
          .iter()
          .find(|p| p.container == http_proxy.container_port)
      })
      .map(|p| p.host);
    let Some(host_port) = host_port else {
      warn!(
        "build_ingress_routes: no host port for service {} container port {} on stack {}, skipping",
        http_proxy.service, http_proxy.container_port, stack.name
      );
      continue;
    };
    routes.push(CaddyRoute {
      hostname: format!("{}.{}", http_proxy.subdomain, base_domain),
      target_endpoint_id: endpoint_id,
      target_port: host_port,
    });
  }
```

- [ ] **Step 2: Build to verify**

```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo build 2>&1 | tail -8
```
Expected: `Finished dev profile`. If `Stack` type import conflicts, adjust the `use` path.

- [ ] **Step 3: Commit**

```bash
git add bin/core/src/resource/deployment.rs
git commit -m "feat(ingress): include stack http_proxy routes in Caddy config

build_ingress_routes now queries both deployments and stacks with
http_proxy configured, emitting CaddyRoute entries for each. Every
Caddy rebuild (on any deploy/delete) includes the full combined route
set."
```

---

### Task 4: Add `try_setup_stack_ingress` + `try_teardown_stack_ingress`

**Covers:** S5

**Files:**
- Modify: `bin/core/src/resource/stack.rs` (add both functions)

**Interfaces:**
- Consumes: `create_stack_dns_record`, `delete_stack_dns_records` from `crate::ingress::management`; `select_new_ingress_node` from `crate::ingress::failover`; `build_ingress_routes`, `build_caddy_config`, `DEFAULT_BRIDGE_PORT` from `crate::resource::deployment`; `periphery_client`, `get_server_for_command`, `core_config`, `server_status_cache`, `db_client` from crate helpers/state; `ReloadCaddyConfig` from `periphery_client::api`.
- Produces: `try_setup_stack_ingress(stack, http_proxy) -> Result<()>` and `try_teardown_stack_ingress(stack_id) -> Result<()>` — called by Task 5 (DeployStack lifecycle) + stack post_delete.

- [ ] **Step 1: Add `try_setup_stack_ingress`**

In `bin/core/src/resource/stack.rs`, at the end of the file (after `validate_config`), add. This mirrors `try_setup_ingress` at `deployment.rs:544`:

```rust
use crate::ingress::{
  failover::select_new_ingress_node,
  management::create_stack_dns_record,
};

/// Set up DNS record + Caddy route for a stack's http_proxy.
/// Mirrors `try_setup_ingress` for deployments.
pub async fn try_setup_stack_ingress(
  stack: &Stack,
  http_proxy: &StackHttpProxyConfig,
) -> anyhow::Result<()> {
  use komodo_client::entities::stack::StackHttpProxyConfig;

  let core_cfg = core_config().ingress.clone();
  let base_domain = core_cfg
    .dns
    .base_domain
    .as_deref()
    .filter(|d| !d.is_empty())
    .ok_or_else(|| {
      anyhow::anyhow!(
        "ingress.dns.base_domain not configured — cannot set up ingress"
      )
    })?;
  let fqdn =
    format!("{}.{}", http_proxy.subdomain, base_domain);

  let server_id = if !stack.config.server_id.is_empty() {
    &stack.config.server_id
  } else {
    &stack.info.assigned_server
  };
  let server = get_server_for_command(server_id).await?;
  let target_endpoint_id = server.info.endpoint_id.clone();
  if target_endpoint_id.is_empty() {
    anyhow::bail!(
      "Server {} has no endpoint_id — cannot route ingress traffic",
      server.id
    );
  }

  // Find the target host port for the proxied service.
  let host_port = stack
    .info
    .host_ports
    .get(&http_proxy.service)
    .and_then(|ports| {
      ports
        .iter()
        .find(|p| p.container == http_proxy.container_port)
    })
    .map(|p| p.host)
    .ok_or_else(|| {
      anyhow::anyhow!(
        "No host port found for service {} container port {} on stack {}. \
         ReadContainerPorts readback may not have completed.",
        http_proxy.service,
        http_proxy.container_port,
        stack.name
      )
    })?;

  let ingress_node = select_new_ingress_node("").await?;

  let cache_entry =
    crate::state::server_status_cache().get(&ingress_node.id).await;
  let (target_ipv4, target_ipv6) = cache_entry
    .as_ref()
    .and_then(|s| s.periphery_info.as_ref())
    .map(|info| (info.public_ipv4.clone(), info.public_ipv6.clone()))
    .unwrap_or((None, None));

  if target_ipv4.is_none() && target_ipv6.is_none() {
    anyhow::bail!(
      "ingress node {} has no cached public_ipv4/v6 — \
       wait for the next poll cycle (default ~5-15s), or set \
       PERIPHERY_PUBLIC_IPV4 / _IPV6 on the Periphery host and \
       restart it",
      ingress_node.id
    );
  }

  create_stack_dns_record(
    &stack.id,
    &http_proxy.subdomain,
    &ingress_node.id,
    target_ipv4.as_deref(),
    target_ipv6.as_deref(),
    &core_cfg,
    60,
  )
  .await?;

  let routes =
    crate::resource::deployment::build_ingress_routes(
      base_domain,
      &core_cfg.dns.cloudflare_api_token,
    )
    .await?;
  let caddy_config =
    crate::resource::deployment::build_caddy_config(
      &routes,
      &core_cfg.dns.cloudflare_api_token.clone().unwrap_or_default(),
      crate::resource::deployment::DEFAULT_BRIDGE_PORT,
    );

  let periphery = periphery_client(&ingress_node).await?;
  periphery
    .request(periphery_client::api::ReloadCaddyConfig {
      config: caddy_config,
    })
    .await?;

  info!(
    "Set up ingress for stack {}: {} -> endpoint {}:{}",
    stack.name, fqdn, target_endpoint_id, host_port
  );
  Ok(())
}
```

- [ ] **Step 2: Add `try_teardown_stack_ingress`**

In the same file, add after the setup function:

```rust
use crate::ingress::management::delete_stack_dns_records;

/// Best-effort: delete DNS records + push updated Caddy config
/// (without the deleted route).
async fn try_teardown_stack_ingress(
  stack_id: &str,
) -> anyhow::Result<()> {
  let core_cfg = core_config().ingress.clone();
  delete_stack_dns_records(stack_id, &core_cfg).await?;

  let base_domain = core_cfg
    .dns
    .base_domain
    .as_deref()
    .filter(|d| !d.is_empty())
    .ok_or_else(|| {
      anyhow::anyhow!(
        "ingress.dns.base_domain not configured — cannot rebuild Caddy"
      )
    })?;
  let routes =
    crate::resource::deployment::build_ingress_routes(
      base_domain,
      &core_cfg.dns.cloudflare_api_token,
    )
    .await?;
  let caddy_config =
    crate::resource::deployment::build_caddy_config(
      &routes,
      &core_cfg.dns.cloudflare_api_token.clone().unwrap_or_default(),
      crate::resource::deployment::DEFAULT_BRIDGE_PORT,
    );

  let ingress_node =
    crate::ingress::failover::select_new_ingress_node("").await?;
  let periphery = periphery_client(&ingress_node).await?;
  periphery
    .request(periphery_client::api::ReloadCaddyConfig {
      config: caddy_config,
    })
    .await?;

  info!("Tore down ingress for stack {}", stack_id);
  Ok(())
}
```

- [ ] **Step 3: Verify the referenced functions exist + visibility**

Check that `build_ingress_routes`, `build_caddy_config`, and `DEFAULT_BRIDGE_PORT` in `bin/core/src/resource/deployment.rs` are `pub` (or `pub(crate)`). Run:
```bash
grep -n "pub async fn build_ingress_routes\|pub fn build_caddy_config\|pub const DEFAULT_BRIDGE_PORT\|async fn build_ingress_routes\|fn build_caddy_config\|const DEFAULT_BRIDGE_PORT" bin/core/src/resource/deployment.rs
```
Expected: they may be private (`fn`/`async fn` without `pub`). If so, change them to `pub(crate)` so `stack.rs` can call them. Make those edits.

- [ ] **Step 4: Build to verify**

```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo build 2>&1 | tail -8
```
Expected: `Finished dev profile`. The setup function is `pub` (called from DeployStack in Task 5); teardown is private (called only within stack.rs).

- [ ] **Step 5: Commit**

```bash
git add bin/core/src/resource/stack.rs bin/core/src/resource/deployment.rs
git commit -m "feat(stack): add try_setup_stack_ingress / try_teardown_stack_ingress

Stack parallels of the deployment ingress setup/teardown: create DNS
record keyed by stack_id, rebuild + push Caddy config including all
routes. Makes build_ingress_routes / build_caddy_config /
DEFAULT_BRIDGE_PORT pub(crate) so stack.rs can call them."
```

---

### Task 5: Wire ingress into DeployStack + host-port readback + post_delete

**Covers:** S2, S5

**Files:**
- Modify: `bin/core/src/api/execute/stack.rs:83` (in `DeployStack::resolve`, after `update_info` ~line 283, before `refresh_server_cache` ~line 296)
- Modify: `bin/core/src/resource/stack.rs:437` (in `post_delete`)

**Interfaces:**
- Consumes: `try_setup_stack_ingress` from `crate::resource::stack`; `StackHttpProxyConfig` from `komodo_client::entities::stack`; `ReadContainerPorts` from `periphery_client::api::placement`; `try_teardown_stack_ingress` from same module.
- Produces: Stack deploy now sets up ingress after successful `ComposeUp`; stack delete tears it down.

- [ ] **Step 1: Add host-port readback + ingress setup to DeployStack::resolve**

In `bin/core/src/api/execute/stack.rs`, in `DeployStack::resolve`, after the `update_info.await` block (which ends ~line 293, before `refresh_server_cache` at line 296), insert:

```rust
    // Read back host ports for the proxied service (if http_proxy set),
    // then set up DNS + Caddy ingress. Best-effort.
    if let Some(http_proxy) = &stack.config.http_proxy {
      // Find the container name for the proxied service.
      let container_name = services
        .iter()
        .find(|s| s.service_name == http_proxy.service)
        .map(|s| s.container_name.clone());
      match container_name {
        Some(container_name) => {
          // Read back host ports from the periphery.
          let periphery = periphery_client(&server).await;
          match periphery {
            Ok(p) => {
              match p
                .request(periphery_client::api::placement::ReadContainerPorts {
                  container_name: container_name.clone(),
                })
                .await
              {
                Ok(ports) => {
                  // Persist host_ports into info.
                  use database::mungos::mongodb::bson::doc;
                  if let Err(e) = db_client()
                    .stacks
                    .update_one(
                      doc! { "name": &stack.name },
                      doc! { "$set": {
                        format!("info.host_ports.{}", http_proxy.service): ports
                      }},
                    )
                    .await
                  {
                    warn!(
                      "ReadContainerPorts: failed to persist host_ports for stack {} service {}: {e:#}",
                      stack.name, http_proxy.service
                    );
                  }
                  // Now set up ingress with the freshly-persisted ports.
                  // Reload the stack to get updated info.host_ports.
                  // Simpler: re-fetch the stack from DB.
                  if let Ok(updated_stack) =
                    crate::resource::get::<Stack>(&stack.id).await
                  {
                    if let Err(e) =
                      try_setup_stack_ingress(&updated_stack, http_proxy).await
                    {
                      warn!(
                        "Failed to set up ingress for stack {}: {e:#}",
                        stack.name
                      );
                    }
                  }
                }
                Err(e) => warn!(
                  "ReadContainerPorts: query failed for stack {} service {}: {e:#}",
                  stack.name, http_proxy.service
                ),
              }
            }
            Err(e) => warn!(
              "ReadContainerPorts: failed to connect to periphery for stack {}: {e:#}",
              stack.name
            ),
          }
        }
        None => warn!(
          "Stack {} http_proxy references service '{}' but no such service found in deployed services. Skipping ingress setup.",
          stack.name, http_proxy.service
        ),
      }
    }
```

Add the needed `use` at the top of the file — `use komodo_client::entities::stack::Stack;` and `use crate::resource::stack::try_setup_stack_ingress;` (adjust path as needed). Also `use crate::helpers::periphery_client;` and `use crate::state::db_client;`. Check existing imports to avoid duplicates.

**Note:** The `ReadContainerPorts` response type — check `periphery_client::api::placement::ReadContainerPorts` for the exact response shape. The periphery returns `Vec<AssignedPort>`. Verify by grepping:
```bash
grep -rn "ReadContainerPorts" client/periphery/periphery_client/ | head
```
Adjust the `$set` to match the actual response type if it's not directly a `Vec<AssignedPort>`.

- [ ] **Step 2: Add teardown to post_delete**

In `bin/core/src/resource/stack.rs`, in `post_delete` (line 437), add before the final `Ok(())`:

```rust
    // Best-effort: delete DNS records + push updated Caddy config.
    if resource.config.http_proxy.is_some() {
      if let Err(e) = try_teardown_stack_ingress(&resource.id).await {
        warn!(
          "Failed to tear down ingress for stack {}: {e:#}",
          resource.id
        );
      }
    }
```

- [ ] **Step 3: Build to verify**

```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo build 2>&1 | tail -8
```
Expected: `Finished dev profile`. Fix any import/path errors.

- [ ] **Step 4: Commit**

```bash
git add bin/core/src/api/execute/stack.rs bin/core/src/resource/stack.rs
git commit -m "feat(stack): wire ingress setup into DeployStack + teardown in post_delete

After ComposeUp returns service container names, DeployStack reads
back host ports for the proxied service (if http_proxy set) and sets
up DNS + Caddy. post_delete tears down DNS + rebuilds Caddy."
```

---

### Task 6: Add http_proxy validation + subdomain uniqueness

**Covers:** S8

**Files:**
- Modify: `bin/core/src/resource/stack.rs:447` (in `validate_config`)

**Interfaces:**
- Consumes: `StackHttpProxyConfig` type; `db_client().stacks` + `db_client().deployments` collections for uniqueness query.

- [ ] **Step 1: Add validation to `validate_config`**

In `bin/core/src/resource/stack.rs`, in `validate_config` (line 447), before the final `Ok(())`, add (only when `http_proxy` is `Some` in the partial config):

```rust
  // Validate http_proxy if present.
  if let Some(http_proxy) = &config.http_proxy {
    if http_proxy.service.is_empty() {
      anyhow::bail!("http_proxy.service cannot be empty");
    }
    if http_proxy.subdomain.is_empty() {
      anyhow::bail!("http_proxy.subdomain cannot be empty");
    }
    // DNS-label-safe: lowercase, letters/digits/hyphens, no leading/trailing hyphen, ≤63 chars.
    let sub = &http_proxy.subdomain;
    if sub.len() > 63
      || !sub.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
      || sub.starts_with('-')
      || sub.ends_with('-')
    {
      anyhow::bail!(
        "http_proxy.subdomain must be a valid DNS label: lowercase, alphanumeric or hyphens, \
         no leading/trailing hyphen, ≤63 chars"
      );
    }
    if http_proxy.container_port == 0 {
      anyhow::bail!("http_proxy.container_port must be > 0");
    }
    // Subdomain uniqueness across deployments + stacks.
    use database::mungos::{find::find_collect, mongodb::bson::doc};
    let conflicting_stack: Vec<Stack> = find_collect(
      &db_client().stacks,
      doc! {
        "config.http_proxy.subdomain": sub,
        "_id": { "$ne": &stack_id_for_uniqueness }
      },
      None,
    )
    .await
    .context("failed to query stacks for subdomain uniqueness")?;
    if !conflicting_stack.is_empty() {
      anyhow::bail!(
        "subdomain '{}' is already in use by stack '{}'",
        sub,
        conflicting_stack[0].name
      );
    }
    use komodo_client::entities::deployment::Deployment;
    let conflicting_dep: Vec<Deployment> = find_collect(
      &db_client().deployments,
      doc! { "config.http_proxy.subdomain": sub },
      None,
    )
    .await
    .context("failed to query deployments for subdomain uniqueness")?;
    if !conflicting_dep.is_empty() {
      anyhow::bail!(
        "subdomain '{}' is already in use by deployment '{}'",
        sub,
        conflicting_dep[0].name
      );
    }
  }
```

**Note on `stack_id_for_uniqueness`:** `validate_config` receives `config: &mut PartialStackConfig` and `_id: &str`. The `_id` param is the stack id (rename it from `_id` to `id` in the function signature — check line 447: `async fn validate_config(config: &mut PartialStackConfig, user: &User)` — it may not have an id param; check the caller at line ~263 `validate_create_config` and line ~343 `validate_update_config`). If `validate_config` does NOT receive the stack id, use `""` (empty string) for create-time exclusion — on create, any match is a conflict. For update, pass the id through. Adjust accordingly by checking the actual function signature.

- [ ] **Step 2: Build to verify**

```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo build 2>&1 | tail -8
```
Expected: `Finished dev profile`.

- [ ] **Step 3: Commit**

```bash
git add bin/core/src/resource/stack.rs
git commit -m "feat(stack): validate http_proxy + enforce subdomain uniqueness

Validates service/subdomain/container_port and checks that no other
stack or deployment already claims the same subdomain, preventing
DNS/Caddy route conflicts."
```

---

### Task 7: Add http_proxy config group to the stack UI

**Covers:** S7

**Files:**
- Modify: `ui/src/resources/stack/config/index.tsx`

**Interfaces:**
- Consumes: `Types.StackHttpProxyConfig` from `komodo_client`; `useRead("GetCoreInfo")` for `ingress.dns.base_domain`; service names from `stack.info.deployed_services ?? latest_services`.

- [ ] **Step 1: Add the "HTTP Proxy" config group**

In `ui/src/resources/stack/config/index.tsx`, define a new config group and include it in the `""` groups array in each mode branch (UI Defined / Files On Server / Git Repo), before `...generalCommon`.

Near the top of the component (after `const currServerId = ...` ~line 145), add:

```tsx
  const coreInfo = useRead("GetCoreInfo", {}).data;
  const baseDomain = coreInfo?.ingress_base_domain;
  const ingressEnabled = coreInfo?.ingress_enabled ?? false;
  const serviceNames =
    (stack?.info?.deployed_services ?? stack?.info?.latest_services)?.map(
      (s) => s.service_name,
    ) ?? [];
```

Then define the group (before `let groups`):

```tsx
  const httpProxyGroup: ConfigGroupArgs<Types.StackConfig> = {
    label: "HTTP Proxy",
    hidden: !!currSwarmId,
    fields: {
      http_proxy: (value, set) => {
        const proxy = value as Types.StackHttpProxyConfig | undefined;
        if (!ingressEnabled) {
          return (
            <ConfigItem
              label="HTTP Proxy"
              description="Configure ingress DNS on the Core first (Settings → Core Config → ingress.dns)."
            >
              <Text c="dimmed" fz="sm">
                Ingress is not enabled on the Core.
              </Text>
            </ConfigItem>
          );
        }
        return (
          <ConfigItem
            label="HTTP Proxy"
            description={
              proxy?.subdomain
                ? `Endpoint: https://${proxy.subdomain}.${baseDomain}`
                : "Expose one service via automatic DNS + HTTPS."
            }
          >
            <Stack gap="xs">
              <Select
                label="Service"
                placeholder="Select or type service name"
                data={serviceNames}
                value={proxy?.service}
                onChange={(service) =>
                  set({
                    http_proxy: {
                      service: service || "",
                      subdomain: proxy?.subdomain ?? "",
                      container_port: proxy?.container_port ?? 80,
                    },
                  })
                }
                disabled={disabled}
                searchable
                creatable
                getCreateLabel={(query) => `Use "${query}"`}
              />
              <TextInput
                label="Subdomain"
                placeholder="myapp"
                value={proxy?.subdomain ?? ""}
                onChange={(e) =>
                  set({
                    http_proxy: {
                      service: proxy?.service ?? "",
                      subdomain: e.target.value.toLowerCase(),
                      container_port: proxy?.container_port ?? 80,
                    },
                  })
                }
                disabled={disabled}
                description={
                  proxy?.subdomain
                    ? `→ https://${proxy.subdomain}.${baseDomain}`
                    : `FQDN: {subdomain}.${baseDomain}`
                }
              />
              <NumberInput
                label="Container Port"
                placeholder="80"
                value={proxy?.container_port}
                onChange={(num) =>
                  set({
                    http_proxy: {
                      service: proxy?.service ?? "",
                      subdomain: proxy?.subdomain ?? "",
                      container_port: typeof num === "number" ? num : 0,
                    },
                  })
                }
                disabled={disabled}
                min={1}
              />
            </Stack>
          </ConfigItem>
        );
      },
    },
  };
```

Add imports: `Select, NumberInput` from `@mantine/core` (Select may already be imported; add NumberInput).

- [ ] **Step 2: Include the group in each mode branch**

In the three mode branches (`mode === "Files On Server"`, `"Git Repo"`, `"UI Defined"`), insert `httpProxyGroup,` into the `""` groups array, right before `...generalCommon`. Example for UI Defined (~line 934):

```tsx
        environment,
        httpProxyGroup,
        ...generalCommon,
```

Do the same for Files On Server (after `configFiles,` ~line 766) and Git Repo (after `configFiles,` ~line 884).

- [ ] **Step 3: Build the UI to verify**

```bash
cd ui && npx tsc --noEmit 2>&1 | tail -10
```
Expected: no new type errors. (If `tsc` is slow/unavailable, `npm run build` or check the vite dev server start.)

- [ ] **Step 4: Commit**

```bash
git add ui/src/resources/stack/config/index.tsx
git commit -m "feat(ui): add HTTP Proxy config group to stack config

Adds a Service (combobox with free-text), Subdomain (with live FQDN
preview), and Container Port field. Shows a disabled state when
ingress is not enabled on the Core. Included in all three stack modes."
```

---

### Task 8: Update post_create TODO comment + verify end-to-end build

**Covers:** S2

**Files:**
- Modify: `bin/core/src/resource/stack.rs:286` (update the deferred TODO comment)

- [ ] **Step 1: Update the stale TODO comment**

In `bin/core/src/resource/stack.rs`, the `post_create` TODO at line ~286 said host-port readback is deferred. Now that Task 5 implements it in `DeployStack::resolve`, update the comment to reflect reality:

Replace the TODO block (lines ~286-299) with a brief note:

```rust
    // Host-port readback for the proxied service (if http_proxy is set)
    // is performed in DeployStack::resolve after ComposeUp returns service
    // container names — see bin/core/src/api/execute/stack.rs.
    // post_create only handles the stack resource record; it does not run
    // ComposeUp, so container names are not yet available here.
```

- [ ] **Step 2: Full workspace build + type check**

```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target cargo build 2>&1 | tail -8
```
Expected: `Finished dev profile` with only pre-existing warnings.

```bash
cd ui && npx tsc --noEmit 2>&1 | tail -10
```
Expected: no new errors.

- [ ] **Step 3: Commit**

```bash
git add bin/core/src/resource/stack.rs
git commit -m "docs: update post_create TODO — host-port readback now in DeployStack

The deferred readback is implemented in DeployStack::resolve (Task 5).
post_create predates container-name availability, so the comment now
points to where the actual logic lives."
```

---

## Self-Review (completed by planner)

**1. Spec coverage:**
- S1 (data model) → Task 1 ✓
- S2 (host-port readback) → Tasks 5, 8 ✓
- S3 (DnsRecord generalization) → Task 1 ✓
- S4 (host-port readback) → Task 5 ✓
- S5 (lifecycle) → Tasks 2, 4, 5 ✓
- S6 (Caddy route integration) → Task 3 ✓
- S7 (UI) → Task 7 ✓
- S8 (validation) → Task 6 ✓
- S9 (out of scope) — no task needed (exclusions).

All spec sections covered. No phantom `Covers:` IDs.

**2. Placeholder scan:** No TBD/TODO/FIXME in steps. The one note on "check the actual function signature" in Task 6 is a verification instruction, not a placeholder — the implementer is told exactly what to check and how to handle each outcome.

**3. Type consistency:** `StackHttpProxyConfig` used consistently (Task 1 defines, Task 4/5/6/7 consume). `create_stack_dns_record` / `delete_stack_dns_records` defined in Task 2, consumed in Task 4. `try_setup_stack_ingress` / `try_teardown_stack_ingress` defined Task 4, consumed Task 5. `build_ingress_routes` referenced consistently.
