# Unify Public IP fields + Dual-Stack Egress Discovery

**Status:** approved
**Date:** 2026-07-20
**Branch (proposed):** unify-public-ip

## [S1] Problem and scope

### Problem

The ingress host-IP story today is fragmented across three unrelated
fields and a single-protocol discovery path:

- `PeripheryInformation.public_ip` (`Option<String>`, singular IPv4)
  — auto-discovered by OpenDNS over IPv4 transport only
  (`bin/periphery/src/helpers.rs:303`). Cannot report IPv6, breaks on
  IPv6-only hosts.
- `ServerListItemInfo.public_ip` — projects the singular field into the
  UI server list.
- `ServerConfig.public_ipv4` / `public_ipv6` (`Option<String> × 2`) —
  entirely manual, set via Komodo API partial-config patch. This is the
  **only** source ingress uses for DNS A/AAAA records today.

So we have auto-discovery producing one value the ingress flow ignores,
and a manual field the ingress flow reads but no operator is prompted
to set. None of these coordinate.

### Scope

Unify to a single source of truth: Periphery-side discovery
(dual-stack) + Periphery-side optional override → both flow through
`PeripheryInformation` on every poll → Core's ingress flow reads from
the cached `PeripheryInformation` instead of `ServerConfig`. Drop
`ServerConfig.public_ipv4` / `public_ipv6` entirely (breaking change,
acceptable for the early fork).

### Out of scope

- AWS EC2 `assign_public_ip` / `use_public_ip` (builder config;
  unrelated — different concept).
- `ServerConfig.external_address` (display override for container
  links; stays as-is).
- `ServerInfo` struct is untouched (it doesn't carry public IP today —
  the rename targets only `PeripheryInformation` and
  `ServerListItemInfo`).
- Failover-Caddy-rebuild TODO (`bin/core/src/ingress/failover.rs:49-51`,
  "Task 10") — only the IP-source path changes; the Caddy rebuild was
  already unimplemented.

## [S2] Discovery mechanism

### Replace

`bin/periphery/src/helpers.rs` `resolve_host_public_ip()` (and its
`opendns_resolver()` helper) is replaced with two parallel HTTP-based
resolvers:

- `resolve_host_public_ipv4() -> Option<String>` — GET
  `https://api4.ipify.org`. ipify's `api4.` subdomain only resolves
  over IPv4 (it has only A records, no AAAA), so transport is pinned.
  Response body is the raw IPv4 string.
- `resolve_host_public_ipv6() -> Option<String>` — GET
  `https://api6.ipify.org`. Symmetric: `api6.` has only AAAA records,
  no A. Response body is the raw IPv6 string.

Both run concurrently via `tokio::join!`, each with a 2-second timeout.
`reqwest` is already a `bin/periphery` dependency (used in
`caddy/binary.rs:134, 198`). Implementation sketch:

```rust
pub async fn resolve_host_public_ipv4() -> Option<String> {
  fetch_ip("https://api4.ipify.org").await
}
pub async fn resolve_host_public_ipv6() -> Option<String> {
  fetch_ip("https://api6.ipify.org").await
}

async fn fetch_ip(url: &str) -> Option<String> {
  tokio::time::timeout(Duration::from_secs(2), async {
    reqwest::get(url).await.ok()?.text().await.ok()
  })
  .await
  .ok()
  .flatten()
  .map(|s| s.trim().to_string())
}
```

Delete the `opendns_resolver()` helper and its `OpenDNSResolver` type
alias. `hickory_resolver` becomes unused in `bin/periphery` — if no
other consumer exists, drop the dep from `bin/periphery/Cargo.toml`.
(Audit before deleting.)

### Caching

Replace `bin/periphery/src/state.rs` `host_public_ip() ->
Option<&'static String>` (single OnceLock) with
`host_public_ipv4() -> Option<&'static String>` and
`host_public_ipv6() -> Option<&'static String>` (two OnceLocks,
populated lazily on first call — the override path skips discovery
entirely). Refresh only on Periphery restart — operators who need to
reflect an IP change restart Periphery or set the env override.

### Failure mode (hard requirement for ingress nodes)

Public IP becomes a hard requirement for ingress nodes. At Periphery
startup, if `config.ingress_enabled = true`, the binary explicitly
resolves both IPs before starting Caddy + the HTTP bridge. If both
`public_ipv4` and `public_ipv6` resolve to `None` (neither
auto-discovered nor env-overridden), the Periphery **exits with
`std::process::exit(1)`** after logging:

```text
error!("ingress-enabled Periphery has no public IPv4/IPv6 — \
        set PERIPHERY_PUBLIC_IPV4 / PERIPHERY_PUBLIC_IPV6, \
        or ensure HTTPS egress to ipify works")
```

systemd reports `failed` immediately; operator sees it via
`systemctl status` and reads `journalctl -u periphery -e` for the
error message. No partial operation. Matches the existing
`check_podman_volume_export_import_support()` hard-gate pattern
(`bin/periphery/src/main.rs:95-109`).

The cached values still populate lazily via OnceLock for the
non-ingress path (forward-handler-only Periphery nodes don't need IPs
and don't pay the discovery cost until first poll).

### Reliability

ipify.org is operated by a single provider (Ryan Parman); not
multi-vendor. If single-vendor risk is unpalatable, alternates like
`https://4.ident.me` / `https://6.ident.me` can be added later tried in
sequence. Default proposal: ipify alone, KISS. Add alternates only if
upstream downtime becomes a real issue.

## [S3] PeripheryConfig additions

Add two new fields to `PeripheryConfig`, following the 4-place rule
(struct def, Default impl, `sanitized()`, `periphery_config()` manual
construction at `bin/periphery/src/config.rs:70`) plus Env struct.

### 1. Struct def (`client/core/rs/src/entities/config/periphery.rs`)

```rust
/// Optional manual IPv4 override for ingress traffic.
/// Used when auto-discovery (HTTPS to api4.ipify.org) is
/// unavailable or returns the wrong address.
/// Default: None (auto-discover)
#[serde(default, skip_serializing_if = "Option::is_none")]
pub public_ipv4: Option<String>,

/// Optional manual IPv6 override for ingress traffic.
/// Same semantics as public_ipv4.
/// Default: None (auto-discover)
#[serde(default, skip_serializing_if = "Option::is_none")]
pub public_ipv6: Option<String>,
```

### 2. Default impl

`public_ipv4: None, public_ipv6: None`

### 3. Env struct (same file)

```rust
/// Override `public_ipv4`
pub periphery_public_ipv4: Option<String>,
/// Override `public_ipv6`
pub periphery_public_ipv6: Option<String>,
```

### 4. `periphery_config()` manual construction

`bin/periphery/src/config.rs:70-167`:

```rust
public_ipv4: env.periphery_public_ipv4.or(config.public_ipv4),
public_ipv6: env.periphery_public_ipv6.or(config.public_ipv6),
```

### 5. `sanitized()`

Straight clone (no redaction — IPs are operational data, not
secrets):

```rust
public_ipv4: self.public_ipv4.clone(),
public_ipv6: self.public_ipv6.clone(),
```

### Resolution precedence (the merged result)

At runtime, in the new `host_public_ipv4()` / `host_public_ipv6()`:

```rust
pub async fn host_public_ipv4() -> Option<String> {
  let cfg = periphery_config();
  if let Some(v4) = cfg.public_ipv4.clone() {
    return Some(v4);  // env/config override wins, skip discovery
  }
  // fall back to discovery + cache via OnceLock
  resolve_host_public_ipv4().await
}
```

Symmetric for `host_public_ipv6()`.

### Documentation updates

- `example/deploy/periphery/.env.example` — add commented
  `PERIPHERY_PUBLIC_IPV4` and `PERIPHERY_PUBLIC_IPV6` examples.
- `config/periphery.config.toml` — add documented examples mirroring
  the existing `ingress_enabled` style.

### No Core-side override

`ServerConfig.public_ipv4` / `public_ipv6` is removed entirely (covered
in §4).

## [S4] Wire format change + ServerConfig removal

### Wire format change

`PeripheryInformation` (`client/core/rs/src/entities/server.rs:400-415`):

```rust
// BEFORE:
pub public_ip: Option<String>,

// AFTER:
pub public_ipv4: Option<String>,
pub public_ipv6: Option<String>,
```

Update `bin/periphery/src/api/poll.rs:58` to populate both:

```rust
let (ipv4, ipv6) =
  tokio::join!(host_public_ipv4(), host_public_ipv6());
PeripheryInformation {
  // ... existing fields ...
  public_ipv4: ipv4,
  public_ipv6: ipv6,
}
```

### List-item projection

`ServerListItemInfo` (`client/core/rs/src/entities/server.rs:37-74`):
same rename — `public_ip` → `public_ipv4` + `public_ipv6`. Update Core
consumer `bin/core/src/resource/server.rs:74-87, 116` (tuple destructure
+ struct construction).

### `ServerConfig.public_ipv4` / `public_ipv6` removal

Delete from:

1. Struct def (`client/core/rs/src/entities/server.rs:268-275`) — two
   fields + their `#[serde(...)]`, `#[builder(default)]`,
   `#[partial_default(...)]` attrs.
2. Default impl (lines 358-359).
3. Comments above the struct fields ("Ingress" section header at line
   257-264 — keep the section header for `ingress_enabled`, just
   delete the two IP fields under it).

Since `ServerConfig` uses `derive_builder` + `Partial` macros
(`#[derive(... Builder, Partial ...)]` at line 100), removal from the
struct def propagates automatically to the generated
`ServerConfigBuilder` and `PartialServerConfig`. No additional sites.

### Existing Mongo data

Old `Server` documents with `config.public_ipv4` / `config.public_ipv6`
set will retain those fields in MongoDB, but the deserializer will
silently ignore them. No migration script needed (operator can
manually `db.servers.updateMany({}, { $unset: { "config.public_ipv4":
"", "config.public_ipv6": "" } })` if cleanliness matters; out of
scope).

### UI consumer

`ui/src/resources/server/index.tsx:162` reads
`useServer(id)?.info.public_ip` — update to read `info.public_ipv4`
for the primary display (and optionally `info.public_ipv6` as
secondary).
`ui/public/client/types.d.ts:2137, 5336` is auto-regenerated by
typeshare on next build; no manual edit.

## [S5] Core ingress flow + verification

### Core ingress flow update

`bin/core/src/resource/deployment.rs:544-636` `try_setup_ingress()` —
replace the `ingress_node.config.public_ipv4` / `.public_ipv6` reads
(lines 601-602) with reads from the cached `PeripheryInformation`:

```rust
let cache_entry =
  server_status_cache().get(&ingress_node.id).await;
let (target_ipv4, target_ipv6) =
  cache_entry
    .as_ref()
    .and_then(|s| s.periphery_info.as_ref())
    .map(|info| (info.public_ipv4.clone(), info.public_ipv6.clone()))
    .unwrap_or((None, None));

if target_ipv4.is_none() && target_ipv6.is_none() {
  anyhow::bail!(
    "ingress node {} has no cached public_ipv4/v6 — run \
     `ReloadPeriphery` / wait for the next poll cycle, or set \
     PERIPHERY_PUBLIC_IPV4 / _IPV6 on the Periphery host",
    ingress_node.id
  );
}

create_deployment_dns_record(
  &deployment.id,
  &http_proxy.subdomain,
  &ingress_node.id,
  target_ipv4.as_deref(),
  target_ipv6.as_deref(),
  &core_cfg,
  60,
)
.await?;
```

### Failover

Symmetric change in `bin/core/src/ingress/failover.rs:26-58`
`handle_ingress_failover()`: same cache lookup pattern, same bail with
clear error if both are None.

### Cache staleness note

The cache updates every poll cycle (default 15s via
`refresh_server_cache`). Operators who set `PERIPHERY_PUBLIC_IPV4` and
restart Periphery will see new IPs in Core within one poll. Worst case
wait: `stats_polling_rate` (default 5s) + the poll round-trip. No
explicit cache invalidation needed.

### Verification plan

1. `cargo fmt && cargo build -p komodo_periphery -p komodo_core
   --release` — compile-check both binaries. Set
   `CARGO_TARGET_DIR=/home/acheong/.cargo-target` first.
2. Local e2e:
   - Stand up a Periphery with `PERIPHERY_INGRESS_ENABLED=true` and no
     public IP override. Call `GetPeripheryInformation` on Core and
     verify `public_ipv4` and `public_ipv6` both populate.
   - On a known-good host with both protocols: both fields non-None.
   - On an IPv4-only host: `public_ipv6` is None, process still starts.
   - On a host with no IPv4 + no IPv6 (block ipify): process exits 1
     with the error log.
3. Live e2e against S1 (Core) + S2 (Periphery ingress):
   - Rebuild Periphery binary from this branch, push to S2
     (`45.86.125.236`).
   - Rebuild Core, push to S1 (`ac@luddite.dev`).
   - Mark `ingress_enabled = true` on S2's Server entity via API.
   - Create an http_proxy deployment pointing at a known container
     port.
   - Verify in Cloudflare DNS panel that the resulting
     `lud-testN.duti.dev` record has **both** an A record (S2's v4)
     and AAAA record (S2's v6).
   - Delete the deployment; verify both records are deleted.
4. UI sanity: `km server list` shows the dual fields in the list
   output.

## [S6] Files touched (summary)

| File | Change |
|---|---|
| `bin/periphery/src/helpers.rs` | Replace `resolve_host_public_ip` + `opendns_resolver` with `resolve_host_public_ipv4/v6` + `fetch_ip`. |
| `bin/periphery/src/state.rs` | Replace `host_public_ip` OnceLock with `host_public_ipv4` + `host_public_ipv6`. |
| `bin/periphery/src/api/poll.rs` | Populate `public_ipv4` + `public_ipv6` on `PeripheryInformation`. |
| `bin/periphery/src/main.rs` | Add startup hard-gate: if `ingress_enabled` and both IPs None → exit 1. |
| `bin/periphery/Cargo.toml` | Possibly drop `hickory-resolver` if no other consumer. Audit first. |
| `bin/periphery/src/config.rs` | 4-place: read `periphery_public_ipv4/v6` env, fall through to config fields. |
| `client/core/rs/src/entities/config/periphery.rs` | 4-place: `PeripheryConfig.public_ipv4/v6` + `Env.periphery_public_ipv4/v6` fields, Default, sanitized(). |
| `client/core/rs/src/entities/server.rs` | Rename `PeripheryInformation.public_ip` → `.public_ipv4`/`.public_ipv6`. Same on `ServerListItemInfo`. Remove `ServerConfig.public_ipv4`/`.public_ipv6` (struct + Default + derive macros). |
| `bin/core/src/resource/server.rs` | Tuple destructure + struct construction updated for the rename. |
| `bin/core/src/resource/deployment.rs` | `try_setup_ingress` reads from `server_status_cache`'s `periphery_info` instead of `config`. |
| `bin/core/src/ingress/failover.rs` | `handle_ingress_failover` symmetric cache lookup. |
| `example/deploy/periphery/.env.example` | Add `PERIPHERY_PUBLIC_IPV4` + `PERIPHERY_PUBLIC_IPV6` commented examples. |
| `config/periphery.config.toml` | Add documented examples mirroring `ingress_enabled` style. |
| `ui/src/resources/server/index.tsx` | Read `info.public_ipv4` (and optionally `.public_ipv6`). |
