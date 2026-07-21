# Unify Public IP fields + Dual-Stack Egress Discovery — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use compose:subagent
> (recommended) or compose:execute to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify the three fragmented "public IP" fields
(`PeripheryInformation.public_ip`, `ServerListItemInfo.public_ip`,
`ServerConfig.public_ipv4`/`public_ipv6`) into a single Periphery-side
source of truth with dual-stack HTTP-based egress discovery
(api4/api6.ipify.org) and optional `PERIPHERY_PUBLIC_IPV4`/
`PERIPHERY_PUBLIC_IPV6` env overrides.

**Architecture:** Periphery discovers both IPv4 and IPv6 egress
addresses in parallel via HTTPS to ipify subdomains (transport pinned
per-protocol by DNS A-only / AAAA-only records). Optional env overrides
take precedence; otherwise discovery runs once and is cached via
`OnceCell`. Both fields flow up to Core in every `PeripheryInformation`
poll. Core's ingress flow reads them from the cached poll response
instead of `ServerConfig`. If an ingress-enabled Periphery can determine
neither address at startup, it `exit(1)`s so systemd reports failure.

**Tech Stack:** Rust (workspace, `komodo_core` / `komodo_periphery`
packages), `reqwest` (already a `bin/periphery` dep), `OnceCell`
(already in tree). Shared types in `client/core/rs`. Typeshare moves
the TS types on next build.

## Global Constraints

- Cargo: set `CARGO_TARGET_DIR=/home/acheong/.cargo-target` before
  every `cargo` command (off-tmpfs build dir).
- Cargo: run `cargo fmt` before every commit — enforced by
  `.githooks/pre-commit` (`cargo fmt -- --check`). Repo config
  `core.hooksPath=.githooks` must be set on this clone.
- Package names are `komodo_core` and `komodo_periphery` — not
  `core`/`periphery`.
- Subagents: if dispatching, pass `model: "neuralwatt/glm-5.2-short"`.
- PeripheryConfig field-addition 4-place rule: every new field appears
  in (1) struct def, (2) `Default` impl, (3) `sanitized()`, (4) the
  manual construction in `bin/periphery/src/config.rs:70-167`. Env
  override also goes in the `Env` struct in the same file.
- This is a hard fork in its early stages — breaking changes that
  improve UX/DX are preferred over compatibility shims.
- Don't save secrets/tokens to memory. Use `.env` at project root.

**Compile-ordering note.** Task 3 (PeripheryInformation rename + state
cache + poll populate) is the "bridge" that requires Tasks 1 + 2 to land
first. Task 2 (PeripheryConfig) introduces fields that aren't yet
referenced by any consumer — it compiles standalone. Task 1 alone does
NOT compile (it deletes `resolve_host_public_ip` / `host_public_ip`
without updating the poll populate site). Implementer should land
Tasks 1+2+3 in a single commit, and Task 4 in a follow-on commit.

---

## Task 1: Replace OpenDNS discovery with dual-stack ipify

**Covers:** [S2] (discovery helpers portion)

**Files:**

- Modify: `bin/periphery/src/helpers.rs:268-319` (replace
  `opendns_resolver` + `resolve_host_public_ip` with
  `resolve_host_public_ipv4`/`_ipv6` + shared `fetch_ip`)
- Modify: `bin/periphery/Cargo.toml:37` (drop `hickory-resolver` dep —
  confirmed only consumer is the OpenDNS code being deleted)

**Interfaces:**

- Produces (consumed by Task 3's state cache):

```rust
// bin/periphery/src/helpers.rs
pub async fn resolve_host_public_ipv4() -> Option<String>
pub async fn resolve_host_public_ipv6() -> Option<String>
```

**Note:** This task does NOT touch `bin/periphery/src/state.rs`'s
`host_public_ip()` (deletion there happens in Task 3 alongside the
`PeripheryInformation` rename — landing Task 1 alone breaks the
workspace; landing Tasks 1+2+3 together restores it).

- [ ] **Step 1: Replace the discovery functions in
  `bin/periphery/src/helpers.rs`**

Open `bin/periphery/src/helpers.rs`. Replace the entire block from
line 268 to the end of file (line 319) — the `// Public IP over DNS`
section header, `type OpenDNSResolver`, `opendns_resolver()`, and
`resolve_host_public_ip()` — with:

```rust
// ===========================
//  Public IP over HTTPS (ipify)
// ===========================

/// Resolve the host's public IPv4 egress address by querying
/// `https://api4.ipify.org`. ipify's `api4.` subdomain has only A
/// records (no AAAA), so transport is pinned to IPv4 — this returns
/// the address external IPv4 connections see, which is what we want
/// for DNS A records on the ingress node.
///
/// Caches in `host_public_ipv4()` via `OnceCell` — call that
/// instead of this function directly unless you want a fresh lookup.
pub async fn resolve_host_public_ipv4() -> Option<String> {
  fetch_ip("https://api4.ipify.org").await
}

/// Resolve the host's public IPv6 egress address by querying
/// `https://api6.ipify.org`. Symmetric to `resolve_host_public_ipv4`
/// — `api6.` has only AAAA records, so transport is pinned to IPv6.
pub async fn resolve_host_public_ipv6() -> Option<String> {
  fetch_ip("https://api6.ipify.org").await
}

/// GET the given HTTPS URL and return the response body as a trimmed
/// string. Returns `None` on any network/parse error or 2s timeout.
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

The `use std::{... time::Duration ...}` import at line 1-4 already
covers `Duration`. `reqwest` is already a `bin/periphery` dep — the
fully-qualified `reqwest::get(...)` call works without an explicit
`use reqwest;`.

- [ ] **Step 2: Clean up unused imports in `helpers.rs`**

After Step 1, these imports in the previous OpenDNS section are no
longer used:

- `IpAddr`, `FromStr as _` (from `std::net`/`std::str`) — were used
  for parsing the OpenDNS server IPv4 literals. Safe to remove **if**
  no other code in `helpers.rs` uses them. Grep before deleting:

```bash
cd /home/acheong/Projects/luddite/deploy
rg -n "IpAddr|FromStr" bin/periphery/src/helpers.rs
```

If the only matches are in the deleted OpenDNS block, remove
`net::IpAddr` and `str::FromStr as _` from the `use std::{...}` block
at line 1-4.

- [ ] **Step 3: Drop the `hickory-resolver` dep from
  `bin/periphery/Cargo.toml`**

Edit `bin/periphery/Cargo.toml`. Find line 37
(`hickory-resolver.workspace = true`). Delete the line.

Verify no other periphery source file uses `hickory_resolver`:

```bash
cd /home/acheong/Projects/luddite/deploy
rg -n "hickory" bin/periphery/src/
```

Expected: no matches. If matches found, do **not** drop the dep —
report back.

- [ ] **Step 4: Do not commit yet** — Task 3 makes the workspace
  compile. The commit lands at the end of Task 3 (covering Tasks 1+2+3).

---

## Task 2: Add `public_ipv4` / `public_ipv6` to PeripheryConfig

**Covers:** [S3]

**Files:**

- Modify: `client/core/rs/src/entities/config/periphery.rs`
  (struct def at line 206+; Default impl at line 394+; `sanitized()` at
  line 427+; `Env` struct at line 94+)
- Modify: `bin/periphery/src/config.rs:70-167` (manual construction)
- Modify: `example/deploy/periphery/.env.example` (add documented
  examples)
- Modify: `config/periphery.config.toml` (add documented examples)

**Interfaces:**

- Produces (consumed by Task 3's override-precedence logic in
  `host_public_ipv4`/`_ipv6`):

```rust
// client/core/rs/src/entities/config/periphery.rs
impl PeripheryConfig {
  pub public_ipv4: Option<String>,
  pub public_ipv6: Option<String>,
}
// Env-overridable as:
//   PERIPHERY_PUBLIC_IPV4 / PERIPHERY_PUBLIC_IPV6
```

- [ ] **Step 1: Add fields to the `PeripheryConfig` struct**

Open `client/core/rs/src/entities/config/periphery.rs`. Find the
`ingress_enabled` field near line 357-360 (the last field before the
struct's closing brace). Insert after it, before the closing `}`:

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

- [ ] **Step 2: Add to the `Default` impl**

In the same file, find the `impl Default for PeripheryConfig` block
near line 394. Find `ingress_enabled: false,` near line 422
(the last field assignment). Insert after it:

```rust
      public_ipv4: None,
      public_ipv6: None,
```

- [ ] **Step 3: Add to `sanitized()`**

Same file, find `impl PeripheryConfig` → `pub fn sanitized(&self)` at
line 427. Find `ingress_enabled: self.ingress_enabled,` near line 499
(the last field). Insert after it:

```rust
      public_ipv4: self.public_ipv4.clone(),
      public_ipv6: self.public_ipv6.clone(),
```

- [ ] **Step 4: Add env overrides to the `Env` struct**

Same file, find `pub struct Env` at line 94. Find the last env field
(`pub periphery_vendored_manifest_url: Option<String>,` near line 199).
Insert after it:

```rust
  /// Override `public_ipv4`
  pub periphery_public_ipv4: Option<String>,
  /// Override `public_ipv6`
  pub periphery_public_ipv6: Option<String>,
```

- [ ] **Step 5: Wire into `periphery_config()` manual construction**

Open `bin/periphery/src/config.rs`. Find the manual construction block
starting at line 70 (`PeripheryConfig {`). Find the last field
being constructed (`ingress_enabled: env.periphery_ingress_enabled
.unwrap_or(config.ingress_enabled),` near line 163-165). Insert after
it, before the closing `}`:

```rust
      public_ipv4: env
        .periphery_public_ipv4
        .unwrap_or(config.public_ipv4),
      public_ipv6: env
        .periphery_public_ipv6
        .unwrap_or(config.public_ipv6),
```

- [ ] **Step 6: Add an example to `example/deploy/periphery/.env.example`**

Open `example/deploy/periphery/.env.example`. Append at end of file:

```text
# Optional: manually set the public IPv4 / IPv6 of this Periphery host.
# If unset, Komodo auto-discovers egress IPs via HTTPS to api4.ipify.org
# (IPv4) and api6.ipify.org (IPv6). Set these when running behind NAT
# where the egress IP is not the same as the address Caddy should bind
# DNS records to, OR when ipify is unreachable from the host.
# Required for ingress-enabled nodes: if both unset AND both discovery
# calls fail, Periphery will exit(1) at startup.
# PERIPHERY_PUBLIC_IPV4="203.0.113.10"
# PERIPHERY_PUBLIC_IPV6="2001:db8::1"
```

- [ ] **Step 7: Add an example to `config/periphery.config.toml`**

Open `config/periphery.config.toml`. Find the section documenting
`ingress_enabled` (or similar — search for `ingress_enabled`). Add a
documented example below it referencing the env vars:

```toml
## Optional manual IPv4 override for ingress traffic.
## Used when auto-discovery (HTTPS to api4.ipify.org) is unavailable.
## Env: PERIPHERY_PUBLIC_IPV4
## Default: None (auto-discover)
# public_ipv4 = "203.0.113.10"

## Optional manual IPv6 override. Same semantics as public_ipv4.
## Env: PERIPHERY_PUBLIC_IPV6
# public_ipv6 = "2001:db8::1"
```

- [ ] **Step 8: Compile-check (expected to fail — that's OK)**

```bash
export CARGO_TARGET_DIR=/home/acheong/.cargo-target
cd /home/acheong/Projects/luddite/deploy
cargo build -p komodo_periphery
```

Expected: fails because Task 1 deleted `resolve_host_public_ip` and
Task 2 didn't yet update `bin/periphery/src/api/poll.rs:58` + the
state cache. Task 3 finishes that wiring. Do **not** commit yet.

---

## Task 3: Rename `PeripheryInformation.public_ip`, replace state cache, populate on poll

**Covers:** [S2] (state cache portion), [S4] (wire format change
portion — PeripheryInformation only)

**Files:**

- Modify: `bin/periphery/src/state.rs:218-231` (replace single
  `host_public_ip` OnceCell with `host_public_ipv4` + `host_public_ipv6`,
  with override precedence from Task 2's config)
- Modify: `bin/periphery/src/api/poll.rs:58` (populate both fields on
  `PeripheryInformation` via `tokio::join!`)
- Modify: `client/core/rs/src/entities/server.rs:400-415`
  (`PeripheryInformation` struct: rename `public_ip` to `public_ipv4`
  + `public_ipv6`)

**Interfaces:**

- Produces (consumed by Task 4 `to_list_item`, Task 5 main.rs
  hard-gate, Task 6 Core ingress flow):

```rust
// bin/periphery/src/state.rs
pub async fn host_public_ipv4() -> Option<&'static String>
pub async fn host_public_ipv6() -> Option<&'static String>

// client/core/rs/src/entities/server.rs (PeripheryInformation)
pub public_ipv4: Option<String>,
pub public_ipv6: Option<String>,
```

- [ ] **Step 1: Rename `PeripheryInformation.public_ip` to
  `public_ipv4` + `public_ipv6`**

Open `client/core/rs/src/entities/server.rs`. Find `pub struct
PeripheryInformation` at line ~400. Find the field at line 413-414:

```rust
  /// The host public ip, if it can be resolved.
  pub public_ip: Option<String>,
```

Replace with:

```rust
  /// The host public IPv4, if it could be resolved.
  pub public_ipv4: Option<String>,
  /// The host public IPv6, if it could be resolved.
  pub public_ipv6: Option<String>,
```

- [ ] **Step 2: Replace `host_public_ip()` in
  `bin/periphery/src/state.rs`**

Open `bin/periphery/src/state.rs`. Find the `host_public_ip` function
at lines 218-231. Replace the entire function with two functions that
incorporate override precedence (Task 2's config.public_ipv4/v6 wins,
else run discovery):

```rust
pub async fn host_public_ipv4() -> Option<&'static String> {
  static PUBLIC_IPV4: OnceCell<Option<String>> =
    OnceCell::const_new();
  PUBLIC_IPV4
    .get_or_init(|| async {
      // Override from config takes precedence — skip discovery if set.
      let cfg = periphery_config();
      if let Some(v4) = cfg.public_ipv4.clone() {
        return Some(v4);
      }
      resolve_host_public_ipv4().await.or_else(|| {
        warn!("Failed to resolve host public IPv4 via ipify");
        None
      })
    })
    .await
    .as_ref()
}

pub async fn host_public_ipv6() -> Option<&'static String> {
  static PUBLIC_IPV6: OnceCell<Option<String>> =
    OnceCell::const_new();
  PUBLIC_IPV6
    .get_or_init(|| async {
      // Override from config takes precedence — skip discovery if set.
      let cfg = periphery_config();
      if let Some(v6) = cfg.public_ipv6.clone() {
        return Some(v6);
      }
      resolve_host_public_ipv6().await.or_else(|| {
        warn!("Failed to resolve host public IPv6 via ipify");
        None
      })
    })
    .await
    .as_ref()
}
```

The `periphery_config()` import is already present at
`bin/periphery/src/state.rs` (look for `use crate::config::periphery_config;`
or a fully-qualified path). The `warn!` macro is available via the
`tracing` crate (already used elsewhere in the file — verify with
`rg "warn!|info!" bin/periphery/src/state.rs`). Update the `use` block:
remove `host_public_ip` import if present anywhere; add
`resolve_host_public_ipv4, resolve_host_public_ipv6` to the
`use crate::helpers::{...}` import.

- [ ] **Step 3: Update `bin/periphery/src/api/poll.rs:58` to populate
  both fields**

Open `bin/periphery/src/api/poll.rs`. Find line 58:

```rust
    public_ip: host_public_ip().await.cloned(),
```

Replace with:

```rust
    public_ipv4: host_public_ipv4().await.cloned(),
    public_ipv6: host_public_ipv6().await.cloned(),
```

Update the `use` block (line ~12) that imports `host_public_ip` from
`crate::state` to instead import `host_public_ipv4, host_public_ipv6`.

- [ ] **Step 4: Compile-check periphery in isolation**

```bash
export CARGO_TARGET_DIR=/home/acheong/.cargo-target
cd /home/acheong/Projects/luddite/deploy
cargo build -p komodo_periphery
```

Expected: `komodo_periphery` builds cleanly. (`komodo_core` will fail
because `ServerListItemInfo.public_ip` still exists — fixed in Task 4.)

- [ ] **Step 5: Commit Tasks 1+2+3 together**

```bash
cd /home/acheong/Projects/luddite/deploy
cargo fmt
git add bin/periphery/src/helpers.rs \
        bin/periphery/Cargo.toml \
        bin/periphery/src/state.rs \
        bin/periphery/src/api/poll.rs \
        bin/periphery/src/config.rs \
        client/core/rs/src/entities/config/periphery.rs \
        client/core/rs/src/entities/server.rs \
        example/deploy/periphery/.env.example \
        config/periphery.config.toml
git commit -m "feat(periphery): dual-stack ipify discovery + PeripheryConfig override

- Replace OpenDNS-over-IPv4 with parallel HTTPS calls to
  api4.ipify.org and api6.ipify.org. Transport pinned per-protocol
  by ipify's A-only / AAAA-only DNS. 2s timeout each. hickory-resolver
  dropped (was the only consumer).
- PeripheryConfig gains public_ipv4/public_ipv6 fields + env overrides
  PERIPHERY_PUBLIC_IPV4/_IPV6 (4-place rule + Env struct). Overrides
  take precedence; otherwise discovery runs once and caches via
  OnceCell.
- PeripheryInformation now carries public_ipv4 + public_ipv6 (singular
  public_ip dropped).

Breaking change for API consumers (early fork — preferred over shims)."
```

---

## Task 4: Rename `ServerListItemInfo.public_ip` + remove `ServerConfig.public_ipv4`/`public_ipv6`

**Covers:** [S4] (ServerListItemInfo rename + ServerConfig removal + UI
consumer)

**Files:**

- Modify: `client/core/rs/src/entities/server.rs:257-275` (delete 2
  fields + attrs from `ServerConfig` struct def) and `:333-360` (Default
  impl cleanup)
- Modify: `client/core/rs/src/entities/server.rs:37-74` (rename
  `ServerListItemInfo.public_ip` → `public_ipv4`/`public_ipv6`)
- Modify: `bin/core/src/resource/server.rs:74-87,116` (tuple destructure
  + struct construction)
- Modify: `ui/src/resources/server/index.tsx` (UI consumer)
- Modify: `ui/public/client/types.d.ts` (regenerated — see Step 6)

**Interfaces:**

- Produces: a compiling workspace with `public_ip` removed everywhere
  on `ServerListItemInfo`/`PeripheryInformation`, and no
  `ServerConfig.public_ipv4`/`public_ipv6`.

- [ ] **Step 1: Remove `public_ipv4`/`public_ipv6` from `ServerConfig`
  struct def**

Open `client/core/rs/src/entities/server.rs`. Find the "Ingress" section
of `ServerConfig` — lines 257-275. Delete these two blocks:

```rust
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

Leave the `ingress_enabled` field above (lines 260-264) intact — only
the two IP fields go.

- [ ] **Step 2: Remove from `ServerConfig::Default` impl**

Same file, near lines 358-359 in `impl Default for ServerConfig`. Delete
these two lines:

```rust
      public_ipv4: None,
      public_ipv6: None,
```

- [ ] **Step 3: Rename `ServerListItemInfo.public_ip` to
  `public_ipv4`+`public_ipv6`**

Same file, find `pub struct ServerListItemInfo` at line 37. Find line
51:

```rust
  /// Host public ip, if it could be resolved.
  pub public_ip: Option<String>,
```

Replace with:

```rust
  /// Host public IPv4, if it could be resolved.
  pub public_ipv4: Option<String>,
  /// Host public IPv6, if it could be resolved.
  pub public_ipv6: Option<String>,
```

- [ ] **Step 4: Update the consumer in
  `bin/core/src/resource/server.rs`**

Open `bin/core/src/resource/server.rs`. Find the tuple destructure in
`ServerResource::to_list_item` around lines 71-87. Replace:

```rust
    let (
      version,
      endpoint_id,
      public_ip,
      terminals_disabled,
      container_terminals_disabled,
    ) = match status.as_ref().and_then(|s| s.periphery_info.as_ref())
    {
      Some(info) => (
        Some(info.version.clone()),
        Some(info.endpoint_id.clone()),
        info.public_ip.clone(),
        info.terminals_disabled,
        info.container_terminals_disabled,
      ),
      None => (None, None, None, true, true),
    };
```

With:

```rust
    let (
      version,
      endpoint_id,
      public_ipv4,
      public_ipv6,
      terminals_disabled,
      container_terminals_disabled,
    ) = match status.as_ref().and_then(|s| s.periphery_info.as_ref())
    {
      Some(info) => (
        Some(info.version.clone()),
        Some(info.endpoint_id.clone()),
        info.public_ipv4.clone(),
        info.public_ipv6.clone(),
        info.terminals_disabled,
        info.container_terminals_disabled,
      ),
      None => (None, None, None, None, true, true),
    };
```

Then at line 116 (the `ServerListItemInfo` struct construction), replace
the single `public_ip,` with:

```rust
        public_ipv4,
        public_ipv6,
```

- [ ] **Step 5: Update the UI consumer in
  `ui/src/resources/server/index.tsx`**

Open `ui/src/resources/server/index.tsx`. Find line 162:

```tsx
      const publicIp = useServer(id)?.info.public_ip;
```

Replace with (use `public_ipv4` for the primary display; surface
`public_ipv6` as a secondary tooltip if straightforward):

```tsx
      const publicIpv4 = useServer(id)?.info.public_ipv4;
      const publicIpv6 = useServer(id)?.info.public_ipv6;
```

Then grep for downstream uses:

```bash
cd /home/acheong/Projects/luddite/deploy
rg -n "publicIp\b" ui/src/
```

Update each downstream site to read `publicIpv4` for primary display.
If there's a single obvious place to show the v6 address (tooltip on
the same UI element), do so. If unclear, surface `publicIpv4` only and
leave a `// TODO: also display publicIpv6` comment for follow-up.

- [ ] **Step 6: Regenerate `ui/public/client/types.d.ts`**

Try the project's typeshare regen step:

```bash
cd /home/acheong/Projects/luddite/deploy
# Try this; if no typeshare step is configured, skip — UI build will
# regenerate as part of `npm run build`.
cargo run -p komodo_client --bin tsync 2>/dev/null || true
```

Verify the regenerated `types.d.ts` no longer has
`public_ip?: string;` (lines 2137, 5336 before) but has
`public_ipv4?: string;` and `public_ipv6?: string;` instead:

```bash
rg -n "public_ip\b|public_ipv4|public_ipv6" ui/public/client/types.d.ts
```

- [ ] **Step 7: Compile-check the whole workspace**

```bash
export CARGO_TARGET_DIR=/home/acheong/.cargo-target
cd /home/acheong/Projects/luddite/deploy
cargo build -p komodo_periphery -p komodo_core
```

If `bin/cli/src/command/list.rs:849`'s
`impl PrintTable for ResourceListItem<ServerListItemInfo>` fails on a
`.public_ip` field reference, update it to read `.public_ipv4`. If it
uses serde derive for column generation (most likely), no manual fix
needed — the rename propagates automatically.

- [ ] **Step 8: Commit**

```bash
cd /home/acheong/Projects/luddite/deploy
cargo fmt
git add client/core/rs/src/entities/server.rs \
        bin/core/src/resource/server.rs \
        ui/src/resources/server/index.tsx \
        ui/public/client/types.d.ts
# Plus any bin/cli files touched if Step 7 needed them:
# git add bin/cli/src/command/list.rs
git commit -m "feat(core,ui): rename ServerListItemInfo.public_ip to dual-stack fields

ServerListItemInfo.public_ip → public_ipv4 + public_ipv6 (matches
PeripheryInformation from prior commit). ServerConfig.public_ipv4 /
public_ipv6 removed entirely — no Core-side manual overrides; the
single source of truth is the Periphery poll response cached in
ServerState."
```

---

## Task 5: Periphery startup hard-gate for ingress nodes

**Covers:** [S2] (failure mode portion — exit(1) when both IPs None on
ingress-enabled nodes)

**Files:**

- Modify: `bin/periphery/src/main.rs:79-127` (insert hard-gate before the
  two `if config.ingress_enabled` blocks at lines 92 and 106)

**Interfaces:**

- Consumes: `host_public_ipv4()` + `host_public_ipv6()` from Task 3's
  `bin/periphery/src/state.rs`

- [ ] **Step 1: Add the startup hard-gate in `main.rs`**

Open `bin/periphery/src/main.rs`. Find the section starting at line 91
(`// Start HTTP ingress bridge (ingress nodes only)` or similar — may
read as `if config.ingress_enabled { let _ = ... http_bridge ... }`).
Immediately **before** that block (i.e., after the forward-handler
spawn block at lines 79-89 closes with `}`), insert:

```rust
  // ===========
  // = Ingress startup hard-gate =
  // ===========
  // Ingress nodes need a public IP (auto-discovered or env-overridden)
  // to route DNS records to. If both are None at startup, exit non-
  // zero so systemd reports failure rather than silently running an
  // ingress node that can't serve traffic.
  if config.ingress_enabled {
    let (ipv4, ipv6) =
      tokio::join!(host_public_ipv4(), host_public_ipv6());
    if ipv4.is_none() && ipv6.is_none() {
      error!(
        "ingress-enabled Periphery has no public IPv4/IPv6 — \
         set PERIPHERY_PUBLIC_IPV4 / PERIPHERY_PUBLIC_IPV6, \
         or ensure HTTPS egress to api4.ipify.org and \
         api6.ipify.org works"
      );
      std::process::exit(1);
    }
    info!("Ingress startup check OK: ipv4={:?} ipv6={:?}", ipv4, ipv6);
  }
```

- [ ] **Step 2: Add `host_public_ipv4`/`_ipv6` to the imports**

Find the `use` block in `bin/periphery/src/main.rs` referencing state
helpers. Likely a `use crate::state::{...}` statement. Add
`host_public_ipv4, host_public_ipv6` to it. If `host_public_ip` was
imported previously (unlikely — the only consumer was `poll.rs`), drop
it. If no state helper is currently imported, add:

```rust
use crate::state::{host_public_ipv4, host_public_ipv6};
```

Verify with:

```bash
cd /home/acheong/Projects/luddite/deploy
rg -n "host_public_ip\b|host_public_ipv" bin/periphery/src/main.rs
```

- [ ] **Step 3: Compile-check periphery**

```bash
export CARGO_TARGET_DIR=/home/acheong/.cargo-target
cd /home/acheong/Projects/luddite/deploy
cargo build -p komodo_periphery
```

Expected: clean build.

- [ ] **Step 4: Commit**

```bash
cd /home/acheong/Projects/luddite/deploy
cargo fmt
git add bin/periphery/src/main.rs
git commit -m "feat(periphery): hard-fail startup on ingress nodes with no public IP

Ingress-enabled Periphery with both public_ipv4 AND public_ipv6 None
(no env override, ipify unreachable for both protocols) now exits(1)
at startup. systemd reports failure so operators see it via
'systemctl status' rather than silently-running-but-broken ingress.

Matches the existing check_podman_volume_export_import_support()
hard-gate pattern in main.rs:95-109."
```

---

## Task 6: Update Core ingress flow + failover to read from cached PeripheryInformation

**Covers:** [S5] (Core ingress flow update portion)

**Files:**

- Modify: `bin/core/src/resource/deployment.rs:593-636`
  (`try_setup_ingress`)
- Modify: `bin/core/src/ingress/failover.rs:26-58`
  (`handle_ingress_failover`)

**Interfaces:**

- Consumes: `server_status_cache()` (already imported in
  `bin/core/src/ingress/failover.rs:19` — verify; if not imported in
  `bin/core/src/resource/deployment.rs`, add the import in Step 2),
  `PeripheryInformation.public_ipv4`/`public_ipv6` (from Task 3)

- [ ] **Step 1: Update `try_setup_ingress` in `deployment.rs`**

Open `bin/core/src/resource/deployment.rs`. Find the
`create_deployment_dns_record(...)` call at lines 597-606. The call
currently passes `ingress_node.config.public_ipv4.as_deref()` and
`ingress_node.config.public_ipv6.as_deref()` at lines 601-602 — but
`ServerConfig.public_ipv4`/`public_ipv6` no longer exist (removed in
Task 4). Replace the whole block of lines 593-606 (from `let
ingress_node = select_new_ingress_node("").await?;` through the
closing `await?;` of `create_deployment_dns_record`) with:

```rust
  // Select a healthy ingress node.
  let ingress_node = select_new_ingress_node("").await?;

  // Read the ingress node's public IPs from its cached
  // PeripheryInformation (populated on every PollStatus cycle —
  // ~stats_polling_rate cadence, default 5-15s). The IPs used to live
  // on ServerConfig as manual overrides; they now flow Periphery →
  // PeripheryInformation → ServerListItemInfo.
  let cache_entry =
    crate::state::server_status_cache().get(&ingress_node.id).await;
  let (target_ipv4, target_ipv6) = cache_entry
    .as_ref()
    .and_then(|s| s.periphery_info.as_ref())
    .map(|info| {
      (info.public_ipv4.clone(), info.public_ipv6.clone())
    })
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

  // Create DNS record(s) pointing to the ingress node.
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

- [ ] **Step 2: Ensure `server_status_cache` is imported in
  `deployment.rs`**

At the top of `bin/core/src/resource/deployment.rs`, find the `use
crate::state::{...}` block. If `server_status_cache` isn't listed, add
it. If no such `use crate::state::{...}` block exists, use the
fully-qualified `crate::state::server_status_cache()` path in the code
from Step 1 (already shown that way). Verify with:

```bash
cd /home/acheong/Projects/luddite/deploy
rg -n "server_status_cache" bin/core/src/resource/deployment.rs
```

- [ ] **Step 3: Update `handle_ingress_failover` in `failover.rs`**

Open `bin/core/src/ingress/failover.rs`. Find the
`update_dns_records_for_node(...)` call at lines 39-47. The call
currently passes `new_node.config.public_ipv4.as_deref()` and
`new_node.config.public_ipv6.as_deref()` at lines 42-43 — but those
fields no longer exist. Replace the block from `let new_node =
select_new_ingress_node(failed_node_id)` (line 30) through the end of
`update_dns_records_for_node(...).await` (line 47) with:

```rust
  let new_node =
    select_new_ingress_node(failed_node_id)
      .await
      .context("select new ingress node for failover")?;
  info!(
    "Failover: selected new ingress node {} for failed node {}",
    new_node.id, failed_node_id
  );

  // Read new node's public IPs from cache (same pattern as
  // try_setup_ingress in resource/deployment.rs).
  let cache_entry =
    server_status_cache().get(&new_node.id).await;
  let (new_ipv4, new_ipv6) = cache_entry
    .as_ref()
    .and_then(|s| s.periphery_info.as_ref())
    .map(|info| {
      (info.public_ipv4.clone(), info.public_ipv6.clone())
    })
    .unwrap_or((None, None));

  if new_ipv4.is_none() && new_ipv6.is_none() {
    anyhow::bail!(
      "failover target node {} has no cached public_ipv4/v6 — \
       wait for the next poll cycle or set \
       PERIPHERY_PUBLIC_IPV4 / _IPV6 on the Periphery host",
      new_node.id
    );
  }

  // Repoint DNS records to the new ingress node.
  super::management::update_dns_records_for_node(
    failed_node_id,
    &new_node.id,
    new_ipv4.as_deref(),
    new_ipv6.as_deref(),
    ingress_config,
  )
  .await
  .context("update DNS records during failover")?;
```

- [ ] **Step 4: Compile-check the core binary**

```bash
export CARGO_TARGET_DIR=/home/acheong/.cargo-target
cd /home/acheong/Projects/luddite/deploy
cargo build -p komodo_core
```

Expected: clean build.

- [ ] **Step 5: Commit**

```bash
cd /home/acheong/Projects/luddite/deploy
cargo fmt
git add bin/core/src/resource/deployment.rs bin/core/src/ingress/failover.rs
git commit -m "feat(core): ingress + failover read public IPs from cached PeripheryInformation

try_setup_ingress and handle_ingress_failover previously read
ServerConfig.public_ipv4/public_ipv6 (manual API overrides). After
Task 4 removed those fields, they now read public_ipv4/v6 from the
cached PeripheryInformation poll response (the single source of
truth post-unification). Bail with a clear error if both are None."
```

---

## Task 7: Live e2e verification

**Covers:** [S5] (verification plan portion)

**Files:** none (test-only task)

**Interfaces:** none (final verification gate)

- [ ] **Step 1: Full workspace cargo build + fmt**

```bash
export CARGO_TARGET_DIR=/home/acheong/.cargo-target
cd /home/acheong/Projects/luddite/deploy
cargo fmt -- --check
cargo build -p komodo_periphery -p komodo_core --release
```

Expected: zero fmt violations; clean release build of both binaries.

- [ ] **Step 2: Local smoke — periphery binary populates both public
  IPs on an ingress-enabled node**

On a host with both IPv4 and IPv6 egress, run the freshly-built
periphery binary with a minimal `.env`:

```bash
mkdir -p /tmp/periph-smoke
cp $CARGO_TARGET_DIR/release/komodo_periphery /tmp/periph-smoke/
cat > /tmp/periph-smoke/.env <<'EOF'
PERIPHERY_CORE_ENDPOINT_ADDRS=""
PERIPHERY_INGRESS_ENABLED=true
PERIPHERY_ROOT_DIRECTORY=/tmp/periph-smoke
EOF
cd /tmp/periph-smoke
RUST_LOG=info ./komodo_periphery 2>&1 | head -n 40
```

Expected: log line `Ingress startup check OK: ipv4=Some("<v4>") ipv6=Some("<v6>")`. Process stays running. Hit Ctrl-C to stop.

- [ ] **Step 3: Local smoke — gate fires when both lookups fail**

Block ipify via `/etc/hosts` (requires root):

```bash
# Add these two lines to /etc/hosts (use sudo):
# 0.0.0.0 api4.ipify.org
# 0.0.0.0 api6.ipify.org
# Pick a nameserver that NXDOMAINs them if /etc/hosts doesn't override.
```

Run the binary with `PERIPHERY_INGRESS_ENABLED=true` and no override
env vars.

Expected: log line `ingress-enabled Periphery has no public IPv4/IPv6 — ...` and process exits with code 1. Verify:

```bash
echo $?  # right after the process exits — should print 1
```

Remove the `/etc/hosts` entries after.

- [ ] **Step 4: Live e2e on S1 (Core) + S2 (Periphery ingress)**

Details per the spec:

1. Rebuild periphery binary from this branch; ship to S2
   (`root@45.86.125.236`). Use the same `screen` + binary-replacement
   flow established in prior sessions.
2. Rebuild core, ship to S1 (`ac@luddite.dev`, fish shell — wrap
   compound SSH in `bash -c "..."`).
3. Restart both. Check Core logs for `Iroh EndpointId:` confirmation.
   Check Periphery S2 logs for `Ingress startup check OK: ipv4=...
   ipv6=...`.
4. Via Core API, mark S2's Server entity's `config.ingress_enabled =
   true` (it may already be set; verify with `km server list`).
5. Wait one poll cycle (~15s).
6. Create an `http_proxy` deployment on S2 pointing at a known
   container port:

```bash
# Using km CLI (komodo_cli) — adjust to actual flags used by this
# fork's CLI contract.
km deployment create \
  --name lud-test-ingress-dual \
  --server-id <S2-server-id> \
  --image nginx:alpine \
  --network host
km deployment http-proxy set \
  --deployment lud-test-ingress-dual \
  --subdomain lud-test1 \
  --container-port 80
```

7. Wait ~30s for the deployment to settle.
8. Check DNS:

```bash
dig +short lud-test1.duti.dev A
dig +short lud-test1.duti.dev AAAA
```

Expected: both return addresses (S2's v4 + v6 respectively, matching
the values in the periphery log line from step 3).

9. Delete the deployment. Verify both A and AAAA records are gone
(integration tests should clean them up; verify):

```bash
dig +short lud-test1.duti.dev A     # should be empty
dig +short lud-test1.duti.dev AAAA  # should be empty
```

- [ ] **Step 5: UI sanity check**

Visit the Komodo UI server list (`km server list` or web UI). Confirm
the public_ipv4 (and public_ipv6 if displayed) column(s) populate for
S2.

- [ ] **Step 6: Clean up test records**

If anything is left in DNS records (Cloudflare) or the
`dns_records` Mongo collection after deployment delete, clean manually
per the rule "Do NOT modify existing DNS records on duti.dev — only
create/modify test records using subdomains like lud-test1.duti.dev":
delete only the `lud-test1.duti.dev` A/AAAA records.

- [ ] **Step 7: No-op commit (verification-only task)**

Usually nothing to commit here — Tasks 1-6 landed the code. If
verification surfaced a doc fix (e.g. .env.example wording bug), land
it as a separate small commit.

---

## Notes for the implementer

- **Task compile ordering**: Tasks 1+2 alone don't compile (Task 1
  deletes `resolve_host_public_ip`/`host_public_ip` without updating
  poll populate, and Task 2 adds config fields not yet wired). The
  commit covering Tasks 1+2+3 lands at the end of Task 3 Step 5. The
  commit covering Task 4 lands after Step 8.
- `hickory-resolver` drop is safe — `rg "hickory" bin/periphery/src/`
  verifies no other consumer. If the audit surprises you, keep the
  dep and report back.
- `ui/public/client/types.d.ts` is generated; if regenerating it
  requires running the UI build pipeline, defer to that step — don't
  hand-edit.
- `bin/cli/src/command/list.rs:849`'s
  `impl PrintTable for ResourceListItem<ServerListItemInfo>` likely
  uses a serde-driven or builder-driven column generator; the rename
  propagates automatically. If `cargo build -p komodo_cli` fails on
  `.public_ip`, fix the reference directly (and add the file to the
  Task 4 commit).
- The `assign_public_ip` / `use_public_ip` fields on AWS builder
  config are UNRELATED (EC2 launch config) — don't touch them.
- After all tasks land + Task 7 verifies, push to origin (master rule:
  main is canonical). If opening a PR, open as draft only if
  implementation is incomplete (e.g. tests deferred).
