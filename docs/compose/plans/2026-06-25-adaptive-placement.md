# Adaptive Placement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use compose:subagent (recommended) or compose:execute to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add placement scheduling, S3-backed volume lifecycle, and node draining to the Komodo fork so operators no longer manually choose which server runs each deployment.

**Architecture:** Three Core-side subsystems — `placement/` (pick target by port availability), `backup/` (cron + on-demand volume export/import via S3), `server/drain.rs` (orchestrate migrations). Periphery gains 5 new RPCs (`CheckHostPorts`, `ReadContainerPorts`, `BackupVolume`, `RestoreVolume`, `ListVolumeBackups`). Entity types in `client/core/rs/src/entities/` are restructured: typed `Vec<PortMapping>`/`Vec<VolumeMount>` replace free-text strings, Swarm mode is dropped entirely.

**Tech Stack:** Rust (Komodo workspace), `netstat2` crate (port probing), `rust-s3` crate (S3 client), `cron` crate (schedule parsing), `serde_yaml` (compose validation, already a dep), `podman volume export`/`import` CLI (volume data transfer).

## Global Constraints

- **Hard fork — no backward compatibility.** Break types freely; do not add migration shims.
- **Module path:** `github.com/luddite-dev/deploy` is irrelevant here — this is the Komodo Rust workspace. Cargo workspace at repo root.
- **Podman version floor:** `podman volume export` and `podman volume import` must exist. Periphery refuses to start if they don't.
- **Periphery runs on host** (not containerized) — `netstat2` reads `/proc/net/tcp` directly.
- **S3 destination config is global**, lives in Core config, forwarded per-operation to Periphery. Periphery is stateless about backup targets.
- **No test infrastructure exists** in the Komodo workspace. Each task that adds tests must add `[dev-dependencies]` to the relevant `Cargo.toml` and create `tests/` directories.
- **Swarm mode is dropped entirely.** Delete `bin/periphery/src/api/swarm/`, `client/periphery/rs/src/api/swarm.rs`, `client/core/rs/src/entities/swarm.rs`, and `bin/core/src/resource/swarm.rs`. Remove all `swarm_id` fields and `get_swarm_or_server` dual-resolution logic.
- **Entity types live in** `client/core/rs/src/entities/` — typeshare-annotated, shared between Core, Periphery, and the TypeScript UI.
- **Periphery API pattern:** request types in `client/periphery/rs/src/api/<topic>.rs` (derive `Resolve`), enum variants in `bin/periphery/src/api/mod.rs:47` (`PeripheryRequest` enum), handler impls in `bin/periphery/src/api/<topic>.rs` (`impl Resolve<Args> for Type`).
- **Core → Periphery dispatch:** `periphery.request(RequestType { ... }).await` via `bin/core/src/periphery/mod.rs:84`.
- **KomodoResource trait:** `bin/core/src/resource/mod.rs:90`. Each resource type implements it with associated `Config`/`Info`/`PartialConfig` types and hook methods (`validate_create_config`, `post_create`, `validate_update_config`, `post_update`, `pre_delete`, `post_delete`).

---

## File Structure

### New files (Core)

| File | Responsibility |
|------|---------------|
| `bin/core/src/placement/mod.rs` | `pick_target()` — candidate filtering, port probe, spread heuristic |
| `bin/core/src/backup/mod.rs` | `BackupDeploymentVolumes` / `BackupStackVolumes` operations + retention enforcement |
| `bin/core/src/backup/scheduler.rs` | Cron-driven periodic backup background task |
| `bin/core/src/server/drain.rs` | Drain controller — state machine + migration orchestration |

### New files (Periphery client API types)

| File | Responsibility |
|------|---------------|
| `client/periphery/rs/src/api/placement.rs` | `CheckHostPorts`, `ReadContainerPorts` request/response types |
| `client/periphery/rs/src/api/volume_backup.rs` | `BackupVolume`, `RestoreVolume`, `ListVolumeBackups` request/response types |

### New files (Periphery API handlers)

| File | Responsibility |
|------|---------------|
| `bin/periphery/src/api/placement.rs` | `impl Resolve<Args>` for `CheckHostPorts`, `ReadContainerPorts` |
| `bin/periphery/src/api/volume_backup.rs` | `impl Resolve<Args>` for `BackupVolume`, `RestoreVolume`, `ListVolumeBackups` |

### New test files

| File | Responsibility |
|------|---------------|
| `bin/core/src/placement/tests.rs` or `bin/core/tests/placement.rs` | Unit tests for `pick_target` |
| `bin/core/tests/volume_validation.rs` | Stack compose YAML bind-mount rejection tests |
| `bin/periphery/tests/volume_backup.rs` | Integration test for export/import round-trip |

### Modified files (entities)

| File | Changes |
|------|---------|
| `client/core/rs/src/entities/deployment.rs` | Drop `swarm_id`; retype `ports`/`volumes`; add `backup`; add `PortMapping`/`VolumeMount`/`BackupConfig`/`AssignedPort`/`VolumeBackupRecord`/`VolumeBackupInfo`/`MigrationState` structs; extend `DeploymentInfo` |
| `client/core/rs/src/entities/stack.rs` | Drop `swarm_id`; add `backup`; extend `StackInfo` |
| `client/core/rs/src/entities/server.rs` | Add `Draining`/`Drained` to `ServerState`; add `desired_state`/`drain_timeout_seconds` to `ServerConfig`; add `ServerDesiredState` enum |
| `client/core/rs/src/entities/mod.rs` | Remove `pub mod swarm;` |

### Modified files (Core resource handlers)

| File | Changes |
|------|---------|
| `bin/core/src/resource/deployment.rs` | Remove `get_swarm_or_server`; update `validate_config` to call `pick_target`; update `post_create`/`post_update` to set `assigned_server`/`host_ports` |
| `bin/core/src/resource/stack.rs` | Same pattern as deployment |
| `bin/core/src/resource/server.rs` | Add drain-state reconciliation in `post_update` |
| `bin/core/src/resource/mod.rs` | Remove `pub mod swarm;` from module list |
| `bin/core/src/main.rs` | Add `mod placement; mod backup; mod server;` |

### Modified files (Periphery)

| File | Changes |
|------|---------|
| `bin/periphery/src/api/mod.rs` | Add 5 new `PeripheryRequest` enum variants |
| `bin/periphery/src/main.rs` | Add `mod placement; mod volume_backup;` + startup Podman version probe |
| `bin/periphery/src/config.rs` | (No change — S3 config is forwarded per-op, not stored) |

### Modified files (Core config)

| File | Changes |
|------|---------|
| `client/core/rs/src/entities/config/core.rs` (or wherever `CoreConfig`/`Env` lives) | Add `BackupDestination` fields to Core env config |
| `bin/core/src/config.rs` | Wire backup destination from env |

### Deleted files (Swarm removal)

| File | Action |
|------|--------|
| `bin/core/src/resource/swarm.rs` | Delete |
| `bin/periphery/src/api/swarm/` | Delete directory |
| `client/periphery/rs/src/api/swarm.rs` | Delete |
| `client/core/rs/src/entities/swarm.rs` | Delete |

---

### Task 1: Drop Swarm Mode

**Covers:** [S4] (Swarm removal part)

**Files:**
- Delete: `bin/core/src/resource/swarm.rs`
- Delete: `bin/periphery/src/api/swarm/` (directory)
- Delete: `client/periphery/rs/src/api/swarm.rs`
- Delete: `client/core/rs/src/entities/swarm.rs`
- Modify: `client/core/rs/src/entities/mod.rs` — remove `pub mod swarm;`
- Modify: `client/core/rs/src/entities/deployment.rs` — remove `swarm_id` field (line 101) and `swarm_id` from `DeploymentListItemInfo` (line 59)
- Modify: `client/core/rs/src/entities/stack.rs` — remove `swarm_id` field
- Modify: `bin/core/src/resource/mod.rs` — remove `pub mod swarm;` from module declarations
- Modify: `bin/core/src/resource/deployment.rs` — remove `get_swarm_or_server` dual-resolution logic and all `swarm_id` references
- Modify: `bin/core/src/resource/stack.rs` — remove all `swarm_id` references
- Modify: `bin/periphery/src/api/mod.rs` — remove all `swarm::*` imports and Swarm-related `PeripheryRequest` variants
- Modify: `bin/periphery/src/main.rs` — remove `mod swarm` declaration
- Modify: `client/periphery/rs/src/api/mod.rs` — remove `pub mod swarm;`

**Interfaces:**
- Consumes: nothing
- Produces: a workspace with no Swarm references, where `server_id` is the sole deployment target field

- [ ] **Step 1: Delete Swarm entity types**

```bash
rm client/core/rs/src/entities/swarm.rs
```

Remove the module registration in `client/core/rs/src/entities/mod.rs` — find and delete the line `pub mod swarm;`.

- [ ] **Step 2: Delete Swarm Periphery API**

```bash
rm -rf bin/periphery/src/api/swarm/
rm client/periphery/rs/src/api/swarm.rs
```

Remove `pub mod swarm;` from `client/periphery/rs/src/api/mod.rs`.
Remove `mod swarm;` from `bin/periphery/src/main.rs`.

- [ ] **Step 3: Delete Swarm Core resource handler**

```bash
rm bin/core/src/resource/swarm.rs
```

Remove `pub mod swarm;` from `bin/core/src/resource/mod.rs` (around line 56-67 in the module list).

- [ ] **Step 4: Remove Swarm variants from PeripheryRequest enum**

In `bin/periphery/src/api/mod.rs`, remove all lines that import from `swarm::` and all `PeripheryRequest` enum variants that reference Swarm types (e.g. `PollSwarmStatus(PollSwarmStatus)`, `InspectSwarmNode`, `RemoveSwarmNodes`, `UpdateSwarmNode`, `InspectSwarmStack`, `DeploySwarmStack`, `RemoveSwarmStacks`, `InspectSwarmService`, `GetSwarmServiceLog`, `GetSwarmServiceLogSearch`, `CreateSwarmService`, `UpdateSwarmService`, `RollbackSwarmService`, `RemoveSwarmServices`, `InspectSwarmTask`, `InspectSwarmConfig`, `CreateSwarmConfig`, `RotateSwarmConfig`, `RemoveSwarmConfigs`, `InspectSwarmSecret`, `CreateSwarmSecret`, `RotateSwarmSecret`, `RemoveSwarmSecrets`).

Also remove `use ... swarm::*` from the import block at the top of the file.

- [ ] **Step 5: Remove swarm_id from DeploymentConfig and DeploymentListItemInfo**

In `client/core/rs/src/entities/deployment.rs`:
- Delete the `pub swarm_id: String,` field at line 101 in `DeploymentConfig`.
- Delete the `pub swarm_id: String,` field at line 59 in `DeploymentListItemInfo` (if present).
- Remove `swarm_id` from the `Default` impl for `DeploymentConfig` if one exists.
- Remove `swarm_id` from any `PartialDeploymentConfig` struct.

- [ ] **Step 6: Remove swarm_id from StackConfig**

In `client/core/rs/src/entities/stack.rs`:
- Delete the `pub swarm_id: String,` field (around line 314).
- Remove from `Default` impl and `PartialStackConfig` if present.

- [ ] **Step 7: Remove get_swarm_or_server from deployment resource**

In `bin/core/src/resource/deployment.rs`:
- Find the `get_swarm_or_server` function or equivalent logic (around lines 75-78, 277-294, 389-411). Replace all dual-resolution with plain `server_id`-only resolution.
- In `validate_config` (private function at ~line 385), remove the `swarm_id` validation branch. Keep the `server_id` validation but change it from "required" to "optional" — remove the error that fires when `server_id` is empty.
- In `inherit_specific_permissions` (~line 75-78), remove the `swarm_id` branch; keep only the `server_id` branch.

- [ ] **Step 8: Remove swarm_id from stack resource handler**

In `bin/core/src/resource/stack.rs`:
- Remove any `swarm_id` references in validation, `inherit_specific_permissions`, and `setup_stack_execution` (or equivalent).
- Remove the `swarm_id` validation branch; make `server_id` optional.

- [ ] **Step 9: Search for remaining Swarm references and clean up**

```bash
rg -i 'swarm' --type rust -l
```

For each file found, remove the Swarm-specific code. Common spots: `bin/core/src/api/` (execute endpoints for swarm operations), `bin/core/src/state.rs` (`swarm_status_cache`), `client/core/rs/src/entities/` (any remaining swarm references in `mod.rs` or `action.rs` or `permission.rs`).

Remove `swarm_status_cache()` and its `OnceLock` from `bin/core/src/state.rs` if present.

- [ ] **Step 10: Verify the workspace compiles**

Run: `cargo check --workspace 2>&1 | tail -40`
Expected: May show errors in files still referencing Swarm — fix iteratively until `cargo check` passes.

Set environment: `CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp cargo check --workspace`

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "feat: drop Swarm mode entirely

Remove swarm_id fields, Swarm entity types, Swarm Periphery API,
Swarm Core resource handler, and all Swarm-related code paths.
server_id is now the sole deployment target field, and is optional."
```

---

### Task 2: Restructure Port and Volume Config Types

**Covers:** [S4] (typed ports/volumes part)

**Files:**
- Modify: `client/core/rs/src/entities/deployment.rs` — replace `ports: String` (line 239) with `ports: Vec<PortMapping>`; replace `volumes: String` (line 249) with `volumes: Vec<VolumeMount>`; add `backup: Option<BackupConfig>`; add new structs `PortMapping`, `VolumeMount`, `BackupConfig`, `AssignedPort`, `VolumeBackupRecord`, `VolumeBackupInfo`, `MigrationState`; extend `DeploymentInfo`; remove `Conversion` struct (line 415) and `conversions_from_str` (line 422)
- Modify: `client/core/rs/src/entities/stack.rs` — add `backup: Option<BackupConfig>`; extend `StackInfo`
- Modify: `bin/periphery/src/api/container/run.rs` — update `push_conversions` calls (lines 150-160) to use typed `Vec<PortMapping>` / `Vec<VolumeMount>` instead of `conversions_from_str`
- Modify: `bin/periphery/src/helpers.rs` — update or remove `push_conversions` helper (line 87-97) if no longer needed
- Modify: `bin/core/src/resource/deployment.rs` — update any code that references `config.ports` or `config.volumes` as strings
- Modify: `bin/core/src/resource/stack.rs` — update any code that references stack volume/ports as strings

**Interfaces:**
- Consumes: nothing (Task 1 must be complete — `swarm_id` already removed)
- Produces:
  - `PortMapping { container: u16, host: Option<u16> }` in `client/core/rs/src/entities/deployment.rs`
  - `VolumeMount { volume: String, mount_path: String }` in the same file
  - `BackupConfig { schedule: Option<String>, max_backups: u32 }` in the same file
  - `AssignedPort { container: u16, host: u16 }` in the same file
  - `VolumeBackupInfo { s3_key: String, timestamp: i64, size_bytes: u64 }` in the same file
  - `VolumeBackupRecord { s3_key: String, timestamp: i64, size_bytes: u64, checksum: String }` in the same file
  - `MigrationState` enum (`Migrating { target_server_id: String, started_at: i64 }` | `Failed { reason: String, at: i64 }`)
  - `DeploymentInfo` now has: `latest_image_digest` (existing), `assigned_server: String`, `host_ports: Vec<AssignedPort>`, `last_backup: HashMap<String, VolumeBackupRecord>`, `migration_state: Option<MigrationState>`
  - `StackInfo` now has: existing fields + `assigned_server: String`, `host_ports: HashMap<String, Vec<AssignedPort>>`, `last_backup: HashMap<String, VolumeBackupRecord>`, `migration_state: Option<MigrationState>`

- [ ] **Step 1: Add new structs to deployment.rs**

In `client/core/rs/src/entities/deployment.rs`, remove the `Conversion` struct (lines 415-420) and `conversions_from_str` function (lines 422-431). Then add the following structs near the bottom of the file (before any impl blocks):

```rust
#[typeshare]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct PortMapping {
  /// The container port to expose.
  pub container: u16,
  /// The host port to bind. None = container-only, Podman assigns random high port.
  pub host: Option<u16>,
}

#[typeshare]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct VolumeMount {
  /// The named volume (no host paths allowed).
  pub volume: String,
  /// The path inside the container where the volume is mounted.
  pub mount_path: String,
}

#[typeshare]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct BackupConfig {
  /// Cron expression for scheduled backups. None = on-demand only.
  pub schedule: Option<String>,
  /// Maximum number of backups to retain per volume.
  #[serde(default = "default_max_backups")]
  pub max_backups: u32,
}

fn default_max_backups() -> u32 { 7 }

#[typeshare]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct AssignedPort {
  pub container: u16,
  pub host: u16,
}

#[typeshare]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct VolumeBackupInfo {
  pub s3_key: String,
  pub timestamp: i64,
  pub size_bytes: u64,
}

#[typeshare]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct VolumeBackupRecord {
  pub s3_key: String,
  pub timestamp: i64,
  pub size_bytes: u64,
  pub checksum: String,
}

#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum MigrationState {
  Migrating { target_server_id: String, started_at: i64 },
  Failed { reason: String, at: i64 },
}
```

- [ ] **Step 2: Replace ports and volumes fields in DeploymentConfig**

In `client/core/rs/src/entities/deployment.rs`, in the `DeploymentConfig` struct (line 89):
- Replace `pub ports: String,` (line 239) with `pub ports: Vec<PortMapping>,`
- Replace `pub volumes: String,` (line 249) with `pub volumes: Vec<VolumeMount>,`
- Add `pub backup: Option<BackupConfig>,` after the `labels` field (around line 267).

Update the `Default` impl for `DeploymentConfig` if it initializes `ports`/`volumes` as `String::default()` — they should now be `Vec::default()` (which is `vec![]`).

- [ ] **Step 3: Extend DeploymentInfo**

In `client/core/rs/src/entities/deployment.rs`, in the `DeploymentInfo` struct (line 69), add after the existing `latest_image_digest` field:

```rust
pub assigned_server: String,
pub host_ports: Vec<AssignedPort>,
pub last_backup: std::collections::HashMap<String, VolumeBackupRecord>,
pub migration_state: Option<MigrationState>,
```

Update `Default` impl for `DeploymentInfo`.

- [ ] **Step 4: Add backup field and extend StackInfo in stack.rs**

In `client/core/rs/src/entities/stack.rs`:
- Add `pub backup: Option<BackupConfig>,` to `StackConfig` (import `BackupConfig` from `super::deployment` or re-export).
- Add to `StackInfo`: `pub assigned_server: String`, `pub host_ports: std::collections::HashMap<String, Vec<AssignedPort>>`, `pub last_backup: std::collections::HashMap<String, VolumeBackupRecord>`, `pub migration_state: Option<MigrationState>`.

Import the necessary types from `deployment.rs` (they're in the same crate — `use super::deployment::{BackupConfig, AssignedPort, VolumeBackupRecord, MigrationState};`).

- [ ] **Step 5: Update container run.rs to use typed ports/volumes**

In `bin/periphery/src/api/container/run.rs` (the `RunContainer` handler), find the `push_conversions` calls at lines 150-160. Replace the `conversions_from_str(ports)` pattern with direct iteration over `Vec<PortMapping>`:

```rust
// Replace push_conversions call for ports:
for pm in &ports {
  match &pm.host {
    Some(host) => res.push_str(&format!(" -p {host}:{}", pm.container)),
    None => res.push_str(&format!(" -p {}", pm.container)),
  }
}

// Replace push_conversions call for volumes:
for vm in &volumes {
  res.push_str(&format!(" -v {}:{}", vm.volume, vm.mount_path));
}
```

Remove the `conversions_from_str` calls and the `.context("Invalid ports")` / `.context("Invalid volumes")` error handling (no longer needed — types are validated at the type level).

- [ ] **Step 6: Remove or update push_conversions helper**

In `bin/periphery/src/helpers.rs`, check if `push_conversions` (line 87) is used anywhere else. If not, remove it. If yes, leave it but it will be unused after the above change — remove it and let the compiler confirm no other callers.

- [ ] **Step 7: Update Core resource handlers for typed ports/volumes**

Search for any code in `bin/core/src/resource/deployment.rs` and `bin/core/src/resource/stack.rs` that treats `config.ports` or `config.volumes` as strings. Update to work with `Vec<PortMapping>` / `Vec<VolumeMount>`.

```bash
rg 'config.ports|config.volumes' bin/core/src/resource/ --type rust
```

- [ ] **Step 8: Verify compilation**

Run: `CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp cargo check --workspace 2>&1 | tail -40`
Expected: PASS — fix any remaining type mismatches.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat: restructure ports/volumes to typed Vecs, add backup/migration types

Replace free-text String ports/volumes fields with typed
Vec<PortMapping> / Vec<VolumeMount>. Bind mounts are now
unrepresentable by typing. Add BackupConfig, AssignedPort,
VolumeBackupRecord/Info, MigrationState types. Extend
DeploymentInfo and StackInfo with placement and backup state."
```

---

### Task 3: Periphery Port Probe and Container Port Readback

**Covers:** [S5] (probe + readback part)

**Files:**
- Create: `client/periphery/rs/src/api/placement.rs` — request/response types
- Create: `bin/periphery/src/api/placement.rs` — handler impls
- Modify: `client/periphery/rs/src/api/mod.rs` — add `pub mod placement;`
- Modify: `bin/periphery/src/api/mod.rs` — add `placement::*` imports + 2 `PeripheryRequest` variants
- Modify: `bin/periphery/src/main.rs` — add `mod placement;`
- Modify: `bin/periphery/Cargo.toml` — add `netstat2` dependency
- Test: `bin/periphery/tests/placement.rs` (new test file)

**Interfaces:**
- Consumes: `AssignedPort` from `client/core/rs/src/entities/deployment.rs` (Task 2)
- Produces:
  - `CheckHostPorts { ports: Vec<u16> } -> CheckHostPortsResponse { free: Vec<u16> }` — Periphery RPC
  - `ReadContainerPorts { container_name: String } -> ReadContainerPortsResponse { ports: Vec<AssignedPort> }` — Periphery RPC
  - Both are dispatched from Core via `periphery.request(CheckHostPorts { ... }).await`

- [ ] **Step 1: Add netstat2 dependency**

In `bin/periphery/Cargo.toml`, add to `[dependencies]`:

```toml
netstat2 = "0.11"
```

Check the workspace `Cargo.toml` — if there's a `[workspace.dependencies]` section, add it there instead and reference it as `netstat2.workspace = true` in `bin/periphery/Cargo.toml`.

- [ ] **Step 2: Create placement.rs in periphery client API**

Create `client/periphery/rs/src/api/placement.rs`:

```rust
use komodo_client::entities::deployment::AssignedPort;
use mogh_resolver::Resolve;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Resolve)]
#[response(CheckHostPortsResponse)]
#[error(anyhow::Error)]
pub struct CheckHostPorts {
  pub ports: Vec<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CheckHostPortsResponse {
  /// The subset of requested ports that are free (not bound by any listener).
  pub free: Vec<u16>,
}

//

#[derive(Debug, Clone, Serialize, Deserialize, Resolve)]
#[response(ReadContainerPortsResponse)]
#[error(anyhow::Error)]
pub struct ReadContainerPorts {
  /// The container name (for Deployment) or project name (for Stack).
  pub container_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReadContainerPortsResponse {
  pub ports: Vec<AssignedPort>,
}
```

- [ ] **Step 3: Register module in periphery client API mod.rs**

In `client/periphery/rs/src/api/mod.rs`, add:

```rust
pub mod placement;
```

- [ ] **Step 4: Add PeripheryRequest enum variants**

In `bin/periphery/src/api/mod.rs`:
- Add `use placement::{CheckHostPorts, ReadContainerPorts};` to the import block (note: the import path may be `periphery_client::api::placement::*` depending on how the file is structured — match the existing import style).
- Add to the `PeripheryRequest` enum:

```rust
// Placement (Read)
CheckHostPorts(CheckHostPorts),
ReadContainerPorts(ReadContainerPorts),
```

- [ ] **Step 5: Create placement handler in Periphery**

Create `bin/periphery/src/api/placement.rs`:

```rust
use mogh_resolver::Resolve;
use netstat2::{get_sockets_info, AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo};
use komodo_client::entities::deployment::AssignedPort;
use crate::helpers::run_komodo_standard_command;

use periphery_client::api::placement::{
  CheckHostPorts, CheckHostPortsResponse,
  ReadContainerPorts, ReadContainerPortsResponse,
};
use crate::Args;

impl Resolve<Args> for CheckHostPorts {
  #[instrument("check_host_ports")]
  async fn resolve(self, _args: &Args) -> anyhow::Result<CheckHostPortsResponse> {
    let sockets = get_sockets_info(
      AddressFamilyFlags::all(),
      ProtocolFlags::TCP,
    )?;
    let bound: std::collections::HashSet<u16> = sockets
      .into_iter()
      .filter_map(|s| match s.protocol_socket_info {
        ProtocolSocketInfo::Tcp(tcp) if tcp.local_port > 0 => Some(tcp.local_port),
        _ => None,
      })
      .collect();
    let free = self.ports.into_iter().filter(|p| !bound.contains(p)).collect();
    Ok(CheckHostPortsResponse { free })
  }
}

impl Resolve<Args> for ReadContainerPorts {
  #[instrument("read_container_ports")]
  async fn resolve(self, _args: &Args) -> anyhow::Result<ReadContainerPortsResponse> {
    let output = run_komodo_standard_command(
      "Read Container Ports",
      &format!("docker inspect --format json {}", self.container_name),
    )?;
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&output)?;
    let mut ports = Vec::new();
    if let Some(container) = parsed.first() {
      if let Some(network) = container.get("NetworkSettings") {
        if let Some(ports_map) = network.get("Ports") {
          if let Some(obj) = ports_map.as_object() {
            for (key, bindings) in obj {
              // key format: "80/tcp" — extract container port
              let container_port: u16 = key
                .split('/')
                .next()
                .unwrap_or("0")
                .parse()
                .unwrap_or(0);
              if let Some(arr) = bindings.as_array() {
                for binding in arr {
                  if let Some(host_port) = binding.get("HostPort").and_then(|v| v.as_str()) {
                    ports.push(AssignedPort {
                      container: container_port,
                      host: host_port.parse().unwrap_or(0),
                    });
                  }
                }
              }
            }
          }
        }
      }
    }
    Ok(ReadContainerPortsResponse { ports })
  }
}
```

- [ ] **Step 6: Register module in Periphery main.rs**

In `bin/periphery/src/main.rs`, add to the module declarations (around line 13-21):

```rust
mod placement;
```

- [ ] **Step 7: Write test for CheckHostPorts**

Create `bin/periphery/tests/placement.rs`:

```rust
use periphery_client::api::placement::CheckHostPorts;

// Integration test — requires a running periphery or direct function test.
// For now, test the netstat2 logic directly.

#[tokio::test]
async fn test_check_host_ports_finds_bound_port() {
  // Port 22 (SSH) is usually bound on test machines, or we bind one ourselves.
  let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
  let bound_port = listener.local_addr().unwrap().port();
  
  let sockets = netstat2::get_sockets_info(
    netstat2::AddressFamilyFlags::all(),
    netstat2::ProtocolFlags::TCP,
  ).unwrap();
  let bound_ports: std::collections::HashSet<u16> = sockets
    .into_iter()
    .filter_map(|s| match s.protocol_socket_info {
      netstat2::ProtocolSocketInfo::Tcp(tcp) => Some(tcp.local_port),
      _ => None,
    })
    .collect();
  
  assert!(bound_ports.contains(&bound_port), "netstat2 should detect the bound port {}", bound_port);
  drop(listener);
}
```

- [ ] **Step 8: Run the test**

Run: `CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp cargo test -p periphery --test placement -- --nocapture`
Expected: PASS

- [ ] **Step 9: Verify workspace compiles**

Run: `CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp cargo check --workspace 2>&1 | tail -20`
Expected: PASS

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat: add CheckHostPorts and ReadContainerPorts Periphery RPCs

CheckHostPorts uses netstat2 to read /proc/net/tcp and return
which requested ports are free. ReadContainerPorts inspects a
running container and returns its host port bindings."
```

---

### Task 4: Core Placement Scheduler

**Covers:** [S5] (algorithm + lifecycle hooks part)

**Files:**
- Create: `bin/core/src/placement/mod.rs` — `pick_target` function + `PlacementError`
- Modify: `bin/core/src/main.rs` — add `mod placement;`
- Modify: `bin/core/src/resource/deployment.rs` — call `pick_target` in `validate_create_config` / `validate_update_config`; set `assigned_server` in `post_create` / `post_update`
- Modify: `bin/core/src/resource/stack.rs` — same pattern
- Modify: `bin/core/src/helpers/mod.rs` — may need a helper to list eligible servers
- Test: `bin/core/tests/placement.rs` (new test file)

**Interfaces:**
- Consumes:
  - `DeploymentConfig` / `StackConfig` from `client/core/rs/src/entities/` (Task 2)
  - `CheckHostPorts` RPC from Task 3
  - `periphery_client()` helper from `bin/core/src/helpers/mod.rs:187`
  - `db_client()` from `bin/core/src/state.rs:34` — to list servers and count deployments per server
- Produces:
  - `pub async fn pick_target(config_ports: &[PortMapping], hint_server_id: &str) -> Result<String, PlacementError>` — returns chosen `server_id`
  - Called from `Deployment::validate_create_config` and `Deployment::validate_update_config`

- [ ] **Step 1: Create placement module**

Create `bin/core/src/placement/mod.rs`:

```rust
use anyhow::Context;
use komodo_client::entities::{
  deployment::PortMapping,
  server::{Server, ServerState},
  resource::Resource,
};
use mungos::find::find_collect;
use crate::{helpers::periphery_client, state::db_client};
use periphery_client::api::placement::CheckHostPorts;

#[derive(Debug, thiserror::Error)]
pub enum PlacementError {
  #[error("hinted server {0} is not available (not healthy or does not exist)")]
  HintedServerUnavailable(String),
  #[error("no eligible server has all required ports free")]
  NoEligibleServer,
  #[error("failed to check ports on server {server_id}: {error}")]
  PortCheckFailed { server_id: String, error: String },
}

/// Pick a target server for a deployment based on port availability.
/// - `config_ports`: the PortMapping list from the deployment config
/// - `hint_server_id`: optional server_id the user pinned (empty = scheduler decides)
/// Returns the chosen server_id.
pub async fn pick_target(
  config_ports: &[PortMapping],
  hint_server_id: &str,
) -> Result<String, PlacementError> {
  // Fixed host ports = ports where host is Some(port)
  let fixed_ports: Vec<u16> = config_ports
    .iter()
    .filter_map(|p| p.host)
    .collect();

  // Get all servers
  let servers: Vec<Server> = find_collect(
    &db_client().servers,
    doc! {},
    None,
  )
  .await
  .map_err(|e| PlacementError::PortCheckFailed {
    server_id: "DB".into(),
    error: e.to_string(),
  })?;

  // Candidates: state == Ok (healthy and not draining/disabled)
  let mut candidates: Vec<&Server> = servers
    .iter()
    .filter(|s| matches!(s.info.state, ServerState::Ok))
    .collect();

  // Sort by fewest assigned deployments (spread heuristic)
  let deployment_counts = count_deployments_per_server().await;
  candidates.sort_by_key(|s| deployment_counts.get(&s.id).copied().unwrap_or(0));

  // If hint is set, check only the hinted server
  if !hint_server_id.is_empty() {
    let hinted = candidates.iter().find(|s| s.id == hint_server_id);
    match hinted {
      Some(server) => {
        let free = check_ports_on_server(server, &fixed_ports).await?;
        if fixed_ports.iter().all(|p| free.contains(p)) {
          return Ok(server.id.clone());
        }
        return Err(PlacementError::HintedServerUnavailable(server.id.clone()));
      }
      None => {
        return Err(PlacementError::HintedServerUnavailable(hint_server_id.to_string()));
      }
    }
  }

  // No hint — probe each candidate
  for server in &candidates {
    let free = check_ports_on_server(server, &fixed_ports).await?;
    if fixed_ports.iter().all(|p| free.contains(p)) {
      return Ok(server.id.clone());
    }
  }

  Err(PlacementError::NoEligibleServer)
}

async fn check_ports_on_server(
  server: &Server,
  ports: &[u16],
) -> Result<Vec<u16>, PlacementError> {
  if ports.is_empty() {
    return Ok(vec![]);
  }
  let periphery = periphery_client(server)
    .await
    .map_err(|e| PlacementError::PortCheckFailed {
      server_id: server.id.clone(),
      error: e.to_string(),
    })?;
  let response = periphery
    .request(CheckHostPorts { ports: ports.to_vec() })
    .await
    .map_err(|e| PlacementError::PortCheckFailed {
      server_id: server.id.clone(),
      error: e.to_string(),
    })?;
  Ok(response.free)
}

async fn count_deployments_per_server() -> std::collections::HashMap<String, u32> {
  // Query all deployments and count by assigned_server
  let deployments: Vec<komodo_client::entities::deployment::Deployment> =
    find_collect(&db_client().deployments, doc! {}, None)
      .await
      .unwrap_or_default();
  let mut counts = std::collections::HashMap::new();
  for d in deployments {
    if !d.info.assigned_server.is_empty() {
      *counts.entry(d.info.assigned_server).or_insert(0) += 1;
    }
  }
  counts
}
```

- [ ] **Step 2: Register module in Core main.rs**

In `bin/core/src/main.rs`, add to the module declarations (around line 11-28):

```rust
mod placement;
```

- [ ] **Step 3: Call pick_target in Deployment validate_config**

In `bin/core/src/resource/deployment.rs`, in the private `validate_config` function (around line 385), after removing the `server_id` required-validation (done in Task 1), add:

```rust
// If server_id is empty (not pinned), the scheduler picks a target.
// If server_id is set (hint), validate it can fit the ports.
// Store the chosen target in the config's server_id temporarily.
let chosen = crate::placement::pick_target(
  &config.ports,
  &config.server_id,
)
.await
.map_err(|e| anyhow::anyhow!("Placement failed: {e}"))?;
// Store as the assignment — post_create will move it to info.assigned_server
config.server_id = chosen;
```

Note: at validate time we set `config.server_id` to the chosen server. In `post_create`/`post_update` we copy it to `info.assigned_server` and may clear the config hint if it was empty. However, since `config.server_id` now holds the actual target and the user may have set it as a hint, we need to track whether the user originally pinned. For simplicity in this plan: `info.assigned_server` is set from `config.server_id` in post_create, and the config retains the pinned hint (or the chosen id if auto-placed).

- [ ] **Step 4: Set assigned_server in Deployment post_create/post_update**

In `bin/core/src/resource/deployment.rs`, in `post_create` (around line 206), add after the existing logic:

```rust
// Set assigned_server from the validated config
let update_doc = doc! { "info.assigned_server": &created.config.server_id };
db_client()
  .deployments
  .update_one_by_id(&created.id, mungos::update::Update::Set(update_doc), None)
  .await
  .context("Failed to set assigned_server")?;
```

In `post_update` (around line 254), add the same pattern.

- [ ] **Step 5: Call pick_target in Stack validate and post_create/post_update**

Apply the same pattern to `bin/core/src/resource/stack.rs`:
- In the stack's `validate_config`, call `pick_target` with the stack's ports (note: stack ports live inside compose YAML, so for this task pass an empty slice — stack port checking will be added when compose validation is implemented in Task 5).
- In `post_create` and `post_update`, set `info.assigned_server`.

- [ ] **Step 6: Write test for pick_target**

Create `bin/core/tests/placement.rs`:

```rust
// Unit tests for placement algorithm.
// These require a running Core with MongoDB, so they are integration tests.
// For pure unit tests, mock the periphery_client and db_client.
// TODO: When mocking infrastructure is added, test:
// - empty hint, all ports free → picks least-loaded server
// - hint set, hinted server has ports free → uses hint
// - hint set, hinted server ports busy → HintedServerUnavailable
// - no eligible server → NoEligibleServer

#[test]
fn placement_error_display() {
  let e = crate::placement::PlacementError::NoEligibleServer;
  assert!(e.to_string().contains("no eligible server"));
}
```

Note: Full integration tests require a running MongoDB + Periphery. The test infrastructure for this will be bootstrapped as part of Task 7 (volume integration tests). For now, verify compilation and manual testing.

- [ ] **Step 7: Verify compilation**

Run: `CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp cargo check --workspace 2>&1 | tail -30`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: add Core placement scheduler with port-based node selection

pick_target probes candidate Periphery nodes' free host ports
via CheckHostPorts RPC. Honors server_id as an optional hint;
fails cleanly with HintedServerUnavailable or NoEligibleServer.
Integrated into Deployment and Stack validate_create_config and
post_create/post_update hooks."
```

---

### Task 5: Stack Compose Validation (Bind Mount + Swarm Key Rejection)

**Covers:** [S6] (validation part)

**Files:**
- Create: `bin/core/src/resource/stack_validation.rs` — compose YAML validator
- Modify: `bin/core/src/resource/stack.rs` — call validator in `validate_config`
- Modify: `bin/core/src/main.rs` or `bin/core/src/resource/mod.rs` — add `mod stack_validation;` (or `pub mod`)
- Test: `bin/core/tests/volume_validation.rs` (new test file)

**Interfaces:**
- Consumes: `StackConfig.file_contents` (the compose YAML string)
- Produces: `pub fn validate_compose_yaml(yaml: &str) -> anyhow::Result<()>` — rejects bind mounts and Swarm-only keys

- [ ] **Step 1: Create stack_validation module**

Create `bin/core/src/resource/stack_validation.rs`:

```rust
use serde_yaml::Value;
use anyhow::{bail, Context};

/// Validate a compose file YAML string.
/// Rejects:
/// - Bind mounts (host paths in service volumes)
/// - Swarm-only compose keys (deploy, replicas, placement, etc.)
pub fn validate_compose_yaml(yaml: &str) -> anyhow::Result<()> {
  let parsed: Value = serde_yaml::from_str(yaml)
    .context("Failed to parse compose file as YAML")?;
  
  let services = parsed
    .get("services")
    .and_then(|s| s.as_mapping())
    .context("Compose file must have a 'services' key")?;

  // Collect declared named volumes from top-level volumes: section
  let declared_volumes: std::collections::HashSet<String> = parsed
    .get("volumes")
    .and_then(|v| v.as_mapping())
    .map(|m| m.keys().filter_map(|k| k.as_str().map(String::from)).collect())
    .unwrap_or_default();

  for (service_name, service_val) in services {
    let service_name = service_name.as_str().unwrap_or("unknown");

    // Reject Swarm-only 'deploy' key
    if service_val.get("deploy").is_some() {
      bail!(
        "Service '{service_name}' uses 'deploy' key, which is Swarm-only. \
         Swarm mode has been removed from this fork."
      );
    }

    // Check volumes
    if let Some(volumes) = service_val.get("volumes").and_then(|v| v.as_sequence()) {
      for vol in volumes {
        let vol_str = match vol {
          Value::String(s) => s.clone(),
          Value::Mapping(m) => {
            // Long form: { type: bind/volume, source: ..., target: ... }
            if let Some(t) = m.get("type").and_then(|v| v.as_str()) {
              if t == "bind" {
                let source = m.get("source").and_then(|v| v.as_str()).unwrap_or("?");
                bail!(
                  "Service '{service_name}' has a bind mount (source: '{source}'). \
                   Only named volumes are allowed."
                );
              }
              continue; // type: volume is OK
            }
            // Fall through to short-form parsing if no type field
            let source = m.get("source").and_then(|v| v.as_str());
            let target = m.get("target").and_then(|v| v.as_str());
            if let (Some(src), Some(_tgt)) = (source, target) {
              if !declared_volumes.contains(src) {
                bail!(
                  "Service '{service_name}' mounts volume '{src}' which is not \
                   declared in the top-level volumes section. Bind mounts are not allowed."
                );
              }
            }
            continue;
          }
          _ => continue,
        };

        // Short form: "source:target" or ":target" or "source:target:mode"
        let parts: Vec<&str> = vol_str.splitn(3, ':').collect();
        if parts.len() >= 2 {
          let source = parts[0];
          if !source.is_empty() && !declared_volumes.contains(source) {
            bail!(
              "Service '{service_name}' has volume '{source}' which is not \
               declared in the top-level volumes section. Bind mounts are not allowed."
            );
          }
          // source empty = anonymous volume, which is fine
        }
      }
    }
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_valid_named_volumes() {
    let yaml = r#"
services:
  web:
    image: nginx
    volumes:
      - data:/var/lib/data
volumes:
  data:
"#;
    assert!(validate_compose_yaml(yaml).is_ok());
  }

  #[test]
  fn test_reject_bind_mount() {
    let yaml = r#"
services:
  web:
    image: nginx
    volumes:
      - /host/path:/container/path
"#;
    let result = validate_compose_yaml(yaml);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("bind mount").or_else(|| {
      // If it's caught by the undeclared-volume path, the message mentions "not declared"
      false
    }) || result.unwrap_err().to_string().contains("not declared"));
  }

  #[test]
  fn test_reject_deploy_key() {
    let yaml = r#"
services:
  web:
    image: nginx
    deploy:
      replicas: 3
"#;
    let result = validate_compose_yaml(yaml);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Swarm-only"));
  }

  #[test]
  fn test_reject_long_form_bind_mount() {
    let yaml = r#"
services:
  web:
    image: nginx
    volumes:
      - type: bind
        source: /host/path
        target: /data
"#;
    let result = validate_compose_yaml(yaml);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("bind mount"));
  }

  #[test]
  fn test_anonymous_volume_ok() {
    let yaml = r#"
services:
  web:
    image: nginx
    volumes:
      - /var/lib/data
"#;
    assert!(validate_compose_yaml(yaml).is_ok());
  }
}
```

- [ ] **Step 2: Register module**

In `bin/core/src/resource/mod.rs`, add to the module declarations:

```rust
pub mod stack_validation;
```

- [ ] **Step 3: Call validator in Stack validate_config**

In `bin/core/src/resource/stack.rs`, in the stack's `validate_config` function (the private validation helper that `validate_create_config` and `validate_update_config` delegate to), add:

```rust
crate::resource::stack_validation::validate_compose_yaml(&config.file_contents)
  .context("Invalid compose file")?;
```

- [ ] **Step 4: Run the tests**

Run: `CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp cargo test -p core --lib resource::stack_validation -- --nocapture`
Expected: All 5 tests PASS

- [ ] **Step 5: Verify workspace compiles**

Run: `CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp cargo check --workspace 2>&1 | tail -20`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: validate Stack compose files — reject bind mounts and Swarm keys

Bind mounts are rejected at validation time. Only named volumes
declared in the top-level volumes section are allowed. Swarm-only
compose keys (deploy, replicas, placement) are rejected. Includes
5 unit tests covering valid configs, bind mounts, long-form binds,
Swarm deploy key, and anonymous volumes."
```

---

### Task 6: Periphery Volume Backup/Restore RPCs

**Covers:** [S6] (export/import/retention part)

**Files:**
- Create: `client/periphery/rs/src/api/volume_backup.rs` — request/response types
- Create: `bin/periphery/src/api/volume_backup.rs` — handler impls
- Modify: `client/periphery/rs/src/api/mod.rs` — add `pub mod volume_backup;`
- Modify: `bin/periphery/src/api/mod.rs` — add `volume_backup::*` imports + 3 `PeripheryRequest` variants
- Modify: `bin/periphery/src/main.rs` — add `mod volume_backup;` + startup Podman version probe
- Modify: `bin/periphery/Cargo.toml` — add `rust-s3` dependency
- Modify: workspace `Cargo.toml` — add `rust-s3` to `[workspace.dependencies]` if using that pattern
- Test: `bin/periphery/tests/volume_backup.rs` (new test file)

**Interfaces:**
- Consumes:
  - `VolumeBackupInfo` / `VolumeBackupRecord` from `client/core/rs/src/entities/deployment.rs` (Task 2)
  - `BackupDestination` config type (added in this task to entity types)
- Produces:
  - `BackupVolume { deployment_id, volume_name, destination } -> BackupResult { s3_key, size_bytes, checksum }`
  - `RestoreVolume { deployment_id, volume_name, source_key, destination } -> RestoreResult { bytes_restored }`
  - `ListVolumeBackups { deployment_id, volume_name, destination } -> Vec<VolumeBackupInfo>`
  - `BackupDestination { endpoint, region, bucket, access_key, secret_key }` struct in entities

- [ ] **Step 1: Add BackupDestination type**

In `client/core/rs/src/entities/deployment.rs` (or a new `backup.rs` entity file), add:

```rust
#[typeshare]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct BackupDestination {
  pub endpoint: String,
  pub region: String,
  pub bucket: String,
  pub access_key: String,
  pub secret_key: String,
}

#[typeshare]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct BackupResult {
  pub s3_key: String,
  pub size_bytes: u64,
  pub checksum: String,
}

#[typeshare]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct RestoreResult {
  pub bytes_restored: u64,
}
```

- [ ] **Step 2: Add rust-s3 dependency**

In the workspace `Cargo.toml`, add to `[workspace.dependencies]`:

```toml
rust-s3 = "0.35"
```

In `bin/periphery/Cargo.toml`, add:

```toml
rust-s3.workspace = true
```

- [ ] **Step 3: Create volume_backup.rs in periphery client API**

Create `client/periphery/rs/src/api/volume_backup.rs`:

```rust
use komodo_client::entities::deployment::{
  BackupDestination, BackupResult, RestoreResult, VolumeBackupInfo,
};
use mogh_resolver::Resolve;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Resolve)]
#[response(BackupResult)]
#[error(anyhow::Error)]
pub struct BackupVolume {
  pub deployment_id: String,
  pub volume_name: String,
  pub destination: BackupDestination,
}

//

#[derive(Debug, Clone, Serialize, Deserialize, Resolve)]
#[response(RestoreResult)]
#[error(anyhow::Error)]
pub struct RestoreVolume {
  pub deployment_id: String,
  pub volume_name: String,
  pub source_key: String,
  pub destination: BackupDestination,
}

//

#[derive(Debug, Clone, Serialize, Deserialize, Resolve)]
#[response(Vec<VolumeBackupInfo>)]
#[error(anyhow::Error)]
pub struct ListVolumeBackups {
  pub deployment_id: String,
  pub volume_name: String,
  pub destination: BackupDestination,
}
```

- [ ] **Step 4: Register module in periphery client API mod.rs**

In `client/periphery/rs/src/api/mod.rs`, add:

```rust
pub mod volume_backup;
```

- [ ] **Step 5: Add PeripheryRequest enum variants**

In `bin/periphery/src/api/mod.rs`, add imports and enum variants:

```rust
use volume_backup::{BackupVolume, RestoreVolume, ListVolumeBackups};
```

In the `PeripheryRequest` enum:

```rust
// Volume Backup
BackupVolume(BackupVolume),
RestoreVolume(RestoreVolume),
ListVolumeBackups(ListVolumeBackups),
```

- [ ] **Step 6: Create volume_backup handler in Periphery**

Create `bin/periphery/src/api/volume_backup.rs`:

```rust
use std::path::Path;
use mogh_resolver::Resolve;
use s3::{Bucket, Credentials, Region, BucketConfiguration};
use komodo_client::entities::deployment::{
  BackupDestination, BackupResult, RestoreResult, VolumeBackupInfo,
};
use crate::helpers::run_komodo_standard_command;
use periphery_client::api::volume_backup::{
  BackupVolume, RestoreVolume, ListVolumeBackups,
};
use crate::Args;

fn build_bucket(dest: &BackupDestination) -> anyhow::Result<Bucket> {
  let creds = Credentials::new(&dest.access_key, &dest.secret_key, None, None, None);
  let region = Region::Custom {
    region: dest.region.clone(),
    endpoint: dest.endpoint.clone(),
  };
  Ok(Bucket::new(&dest.bucket, true, creds)?)
}

fn s3_key_prefix(deployment_id: &str, volume_name: &str) -> String {
  format!("backups/deployments/{deployment_id}/volumes/{volume_name}")
}

impl Resolve<Args> for BackupVolume {
  #[instrument("backup_volume")]
  async fn resolve(self, _args: &Args) -> anyhow::Result<BackupResult> {
    let timestamp = chrono::Utc::now().timestamp();
    let local_file = format!("/tmp/{}-{timestamp}.tar", self.volume_name);
    let s3_key = format!("{}/{timestamp}.tar", s3_key_prefix(&self.deployment_id, &self.volume_name));

    // Export volume to local tarball
    run_komodo_standard_command(
      "Backup Volume",
      &format!("podman volume export {} --output {}", self.volume_name, local_file),
    )?;

    // Upload to S3
    let bucket = build_bucket(&self.destination)?;
    let file_data = std::fs::read(&local_file)?;
    let size_bytes = file_data.len() as u64;
    let checksum = format!("{:x}", md5::compute(&file_data));
    bucket.put_object(&s3_key, &file_data).await?;

    // Clean up local file
    std::fs::remove_file(&local_file)?;

    Ok(BackupResult { s3_key, size_bytes, checksum })
  }
}

impl Resolve<Args> for RestoreVolume {
  #[instrument("restore_volume")]
  async fn resolve(self, _args: &Args) -> anyhow::Result<RestoreResult> {
    let local_file = format!("/tmp/{}-restore.tar", self.volume_name);

    // Download from S3
    let bucket = build_bucket(&self.destination)?;
    let data = bucket.get_object(&self.source_key).await?.to_bytes()?;
    let bytes_restored = data.len() as u64;
    std::fs::write(&local_file, &data)?;

    // Create volume if it doesn't exist
    let _ = run_komodo_standard_command(
      "Create Volume",
      &format!("podman volume create {} 2>/dev/null || true", self.volume_name),
    );

    // Import volume from tarball
    run_komodo_standard_command(
      "Restore Volume",
      &format!("podman volume import {} {}", self.volume_name, local_file),
    )?;

    // Clean up local file
    std::fs::remove_file(&local_file)?;

    Ok(RestoreResult { bytes_restored })
  }
}

impl Resolve<Args> for ListVolumeBackups {
  #[instrument("list_volume_backups")]
  async fn resolve(self, _args: &Args) -> anyhow::Result<Vec<VolumeBackupInfo>> {
    let bucket = build_bucket(&self.destination)?;
    let prefix = format!("{}/", s3_key_prefix(&self.deployment_id, &self.volume_name));
    let results = bucket.list(prefix.as_str(), None).await?;

    let mut backups: Vec<VolumeBackupInfo> = results
      .0
      .into_iter()
      .filter_map(|obj| {
        let key = obj.key;
        // Extract timestamp from filename "backups/.../volumes/<name>/<timestamp>.tar"
        let timestamp = key
          .rsplit('/')
          .next()?
          .strip_suffix(".tar")?
          .parse::<i64>()
          .ok()?;
        Some(VolumeBackupInfo {
          s3_key: key,
          timestamp,
          size_bytes: obj.size as u64,
        })
      })
      .collect();

    backups.sort_by_key(|b| b.timestamp);
    Ok(backups)
  }
}
```

Note: Add `chrono`, `md5` dependencies to `bin/periphery/Cargo.toml` (or workspace deps) if not already present. Check with: `rg 'chrono|md5 ' Cargo.toml bin/periphery/Cargo.toml`.

- [ ] **Step 7: Add Podman version probe at Periphery startup**

In `bin/periphery/src/main.rs`, in the `app()` function (line 23), add at the start:

```rust
// Verify podman volume export/import support
if let Err(e) = std::process::Command::new("podman")
  .args(["volume", "export", "--help"])
  .stdout(std::process::Stdio::null())
  .stderr(std::process::Stdio::null())
  .status()
{
  panic!("Podman is not available: {e}");
}
let export_help = std::process::Command::new("podman")
  .args(["volume", "export", "--help"])
  .output();
match export_help {
  Ok(output) if output.status.success() => {}
  _ => panic!("Podman version does not support 'volume export'. Please upgrade Podman."),
}
let import_help = std::process::Command::new("podman")
  .args(["volume", "import", "--help"])
  .output();
match import_help {
  Ok(output) if output.status.success() => {}
  _ => panic!("Podman version does not support 'volume import'. Please upgrade Podman."),
}
```

- [ ] **Step 8: Register module in Periphery main.rs**

In `bin/periphery/src/main.rs`, add to module declarations:

```rust
mod volume_backup;
```

- [ ] **Step 9: Write test for volume export/import round-trip**

Create `bin/periphery/tests/volume_backup.rs`:

```rust
// Integration test — requires Podman and an S3-compatible store (e.g. MinIO).
// Skipped unless KOMODO_TEST_S3_ENDPOINT env var is set.

fn s3_configured() -> bool {
  std::env::var("KOMODO_TEST_S3_ENDPOINT").is_ok()
}

#[tokio::test]
#[ignore = "requires Podman + S3 (set KOMODO_TEST_S3_ENDPOINT to enable)"]
async fn test_volume_backup_restore_roundtrip() {
  use periphery_client::api::volume_backup::{BackupVolume, RestoreVolume};
  use komodo_client::entities::deployment::BackupDestination;

  let dest = BackupDestination {
    endpoint: std::env::var("KOMODO_TEST_S3_ENDPOINT").unwrap(),
    region: std::env::var("KOMODO_TEST_S3_REGION").unwrap_or_else(|_| "us-east-1".into()),
    bucket: std::env::var("KOMODO_TEST_S3_BUCKET").unwrap(),
    access_key: std::env::var("KOMODO_TEST_S3_ACCESS_KEY").unwrap(),
    secret_key: std::env::var("KOMODO_TEST_S3_SECRET_KEY").unwrap(),
  };

  let volume_name = "komodo-test-roundtrip";
  
  // Create a volume with known content
  std::process::Command::new("podman")
    .args(["volume", "create", volume_name])
    .status()
    .unwrap();
  std::process::Command::new("podman")
    .args(["run", "--rm", "-v", &format!("{volume_name}:/data"), "busybox", "sh", "-c", "echo hello > /data/test.txt"])
    .status()
    .unwrap();

  // Backup
  // Note: this requires a running periphery instance. For a pure unit test,
  // the backup/restore logic should be factored into testable functions.
  // This test is a smoke test for manual CI use.

  // Cleanup
  std::process::Command::new("podman")
    .args(["volume", "rm", volume_name])
    .status()
    .unwrap();
}
```

- [ ] **Step 10: Verify compilation**

Run: `CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp cargo check --workspace 2>&1 | tail -30`
Expected: PASS — fix any dependency/type issues iteratively.

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "feat: add BackupVolume, RestoreVolume, ListVolumeBackups Periphery RPCs

Volume export via 'podman volume export', import via 'podman volume
import'. S3 upload/download via rust-s3 crate. BackupDestination
config forwarded per-operation from Core (Periphery is stateless).
Startup probe rejects unsupported Podman versions."
```

---

### Task 7: Core Backup Operations and Scheduler

**Covers:** [S7] (on-demand + scheduled backup part)

**Files:**
- Create: `bin/core/src/backup/mod.rs` — `backup_deployment_volumes` / `backup_stack_volumes` functions + retention enforcement
- Create: `bin/core/src/backup/scheduler.rs` — cron-driven background task
- Modify: `bin/core/src/main.rs` — add `mod backup;` and spawn scheduler in `app()`
- Modify: `bin/core/src/config.rs` — add `BackupDestination` config from env
- Modify: workspace `Cargo.toml` or `bin/core/Cargo.toml` — add `cron` crate dependency

**Interfaces:**
- Consumes:
  - `BackupVolume`, `RestoreVolume`, `ListVolumeBackups` RPCs from Task 6
  - `periphery_client()` from `bin/core/src/helpers/mod.rs`
  - `db_client()` from `bin/core/src/state.rs`
  - `BackupConfig` / `VolumeBackupRecord` from Task 2
  - `BackupDestination` from Task 6
- Produces:
  - `pub async fn backup_deployment_volumes(deployment_id: &str) -> anyhow::Result<()>` — backs up all volumes, updates `info.last_backup`, enforces retention
  - `pub async fn backup_stack_volumes(stack_id: &str) -> anyhow::Result<()>` — same for stacks
  - `pub fn backup_destination() -> Option<&'static BackupDestination>` — global config accessor
  - Scheduler spawns in `main.rs::app()` and ticks on cron schedules

- [ ] **Step 1: Add cron crate dependency**

In workspace `Cargo.toml` `[workspace.dependencies]`:

```toml
cron = "0.15"
```

In `bin/core/Cargo.toml`:

```toml
cron.workspace = true
```

- [ ] **Step 2: Add backup destination to Core config**

In `bin/core/src/config.rs`, add a `OnceLock`-cached accessor:

```rust
use komodo_client::entities::deployment::BackupDestination;

static BACKUP_DESTINATION: OnceLock<Option<BackupDestination>> = OnceLock::new();

pub fn backup_destination() -> Option<&'static BackupDestination> {
  BACKUP_DESTINATION.get_or_init(|| {
    let endpoint = std::env::var("KOMODO_BACKUP_S3_ENDPOINT").ok()?;
    let region = std::env::var("KOMODO_BACKUP_S3_REGION").ok()?;
    let bucket = std::env::var("KOMODO_BACKUP_S3_BUCKET").ok()?;
    let access_key = std::env::var("KOMODO_BACKUP_S3_ACCESS_KEY").ok()?;
    let secret_key = std::env::var("KOMODO_BACKUP_S3_SECRET_KEY").ok()?;
    Some(BackupDestination { endpoint, region, bucket, access_key, secret_key })
  })
  .as_ref()
}
```

- [ ] **Step 3: Create backup module**

Create `bin/core/src/backup/mod.rs`:

```rust
pub mod scheduler;

use anyhow::Context;
use komodo_client::entities::{
  deployment::{Deployment, VolumeBackupRecord, BackupDestination},
  resource::Resource,
};
use mungos::find::find_collect;
use periphery_client::api::volume_backup::{BackupVolume, ListVolumeBackups};
use crate::{helpers::periphery_client, state::db_client, config::backup_destination};

/// Back up all named volumes for a deployment to S3.
/// Updates info.last_backup and enforces max_backups retention.
pub async fn backup_deployment_volumes(deployment_id: &str) -> anyhow::Result<()> {
  let dest = backup_destination()
    .context("Backup destination not configured (set KOMODO_BACKUP_S3_* env vars)")?
    .clone();

  let deployment: Deployment = db_client()
    .deployments
    .find_one(doc! { "_id": deployment_id })
    .await
    .context("Failed to find deployment")?
    .context("Deployment not found")?;

  // Get the Periphery client for the assigned server
  let server: komodo_client::entities::server::Server = db_client()
    .servers
    .find_one(doc! { "_id": &deployment.info.assigned_server })
    .await?
    .context("Server not found")?;
  let periphery = periphery_client(&server).await?;

  let max_backups = deployment.config.backup
    .as_ref()
    .map(|b| b.max_backups)
    .unwrap_or(7);

  for vm in &deployment.config.volumes {
    let result = periphery
      .request(BackupVolume {
        deployment_id: deployment_id.to_string(),
        volume_name: vm.volume.clone(),
        destination: dest.clone(),
      })
      .await
      .context("BackupVolume RPC failed")?;

    // Update info.last_backup
    let record = VolumeBackupRecord {
      s3_key: result.s3_key,
      timestamp: chrono::Utc::now().timestamp(),
      size_bytes: result.size_bytes,
      checksum: result.checksum,
    };
    db_client()
      .deployments
      .update_one_by_id(
        deployment_id,
        mungos::update::Update::Set(doc! {
          &format!("info.last_backup.{}", vm.volume): mungos::serialize(&record)?
        }),
        None,
      )
      .await?;

    // Enforce retention
    enforce_retention(
      &periphery,
      deployment_id,
      &vm.volume,
      &dest,
      max_backups,
    )
    .await?;
  }

  Ok(())
}

/// Back up all named volumes for a stack (volumes parsed from compose YAML).
pub async fn backup_stack_volumes(stack_id: &str) -> anyhow::Result<()> {
  let dest = backup_destination()
    .context("Backup destination not configured")?
    .clone();

  let stack: komodo_client::entities::stack::Stack = db_client()
    .stacks
    .find_one(doc! { "_id": stack_id })
    .await?
    .context("Stack not found")?;

  let server: komodo_client::entities::server::Server = db_client()
    .servers
    .find_one(doc! { "_id": &stack.info.assigned_server })
    .await?
    .context("Server not found")?;
  let periphery = periphery_client(&server).await?;

  // Parse named volumes from compose YAML
  let volumes = parse_stack_volumes(&stack.config.file_contents)?;

  let max_backups = stack.config.backup
    .as_ref()
    .map(|b| b.max_backups)
    .unwrap_or(7);

  for vol_name in volumes {
    let result = periphery
      .request(BackupVolume {
        deployment_id: stack_id.to_string(),
        volume_name: vol_name.clone(),
        destination: dest.clone(),
      })
      .await?;

    // Update info.last_backup
    let record = VolumeBackupRecord {
      s3_key: result.s3_key,
      timestamp: chrono::Utc::now().timestamp(),
      size_bytes: result.size_bytes,
      checksum: result.checksum,
    };
    db_client()
      .stacks
      .update_one_by_id(
        stack_id,
        mungos::update::Update::Set(doc! {
          &format!("info.last_backup.{vol_name}"): mungos::serialize(&record)?
        }),
        None,
      )
      .await?;

    enforce_retention(&periphery, stack_id, &vol_name, &dest, max_backups).await?;
  }

  Ok(())
}

async fn enforce_retention(
  periphery: &crate::periphery::PeripheryClient,
  deployment_id: &str,
  volume_name: &str,
  dest: &BackupDestination,
  max_backups: u32,
) -> anyhow::Result<()> {
  let backups: Vec<_> = periphery
    .request(ListVolumeBackups {
      deployment_id: deployment_id.to_string(),
      volume_name: volume_name.to_string(),
      destination: dest.clone(),
    })
    .await?;

  if backups.len() as u32 <= max_backups {
    return Ok(());
  }

  // Delete oldest beyond max_backups
  let to_delete = &backups[..backups.len().saturating_sub(max_backups as usize)];
  for backup in to_delete {
    // Delete S3 object directly (need S3 client in Core, or add a DeleteBackup RPC)
    // For now, we delete via a new DeleteS3Object RPC — but to keep this task
    // bounded, we'll delete directly using rust-s3 in Core.
    // This requires adding rust-s3 to bin/core/Cargo.toml too.
    delete_s3_object(dest, &backup.s3_key).await?;
  }

  Ok(())
}

async fn delete_s3_object(dest: &BackupDestination, key: &str) -> anyhow::Result<()> {
  use s3::{Bucket, Credentials, Region};
  let creds = Credentials::new(&dest.access_key, &dest.secret_key, None, None, None);
  let region = Region::Custom {
    region: dest.region.clone(),
    endpoint: dest.endpoint.clone(),
  };
  let bucket = Bucket::new(&dest.bucket, true, creds)?;
  bucket.delete_object(key).await?;
  Ok(())
}

fn parse_stack_volumes(yaml: &str) -> anyhow::Result<Vec<String>> {
  let parsed: serde_yaml::Value = serde_yaml::from_str(yaml)?;
  let volumes = parsed
    .get("volumes")
    .and_then(|v| v.as_mapping())
    .map(|m| {
      m.keys()
        .filter_map(|k| k.as_str().map(String::from))
        .collect()
    })
    .unwrap_or_default();
  Ok(volumes)
}
```

Note: Add `rust-s3` and `serde_yaml` to `bin/core/Cargo.toml` as well.

- [ ] **Step 4: Create scheduler**

Create `bin/core/src/backup/scheduler.rs`:

```rust
use cron::Schedule;
use std::str::FromStr;
use komodo_client::entities::deployment::Deployment;
use mungos::find::find_collect;
use crate::state::db_client;

/// Background task that ticks and fires scheduled backups.
pub async fn run_scheduler() {
  loop {
    // Check every 60 seconds
    tokio::time::sleep(std::time::Duration::from_secs(60)).await;

    if let Err(e) = tick().await {
      tracing::warn!("Backup scheduler tick failed: {e}");
    }
  }
}

async fn tick() -> anyhow::Result<()> {
  let now = chrono::Utc::now();

  // Check deployments with scheduled backups
  let deployments: Vec<Deployment> = find_collect(
    &db_client().deployments,
    doc! {},
    None,
  )
  .await?;

  for deployment in deployments {
    let Some(backup_config) = &deployment.config.backup else {
      continue;
    };
    let Some(cron_expr) = &backup_config.schedule else {
      continue;
    };

    // Skip if currently migrating
    if deployment.info.migration_state.is_some() {
      continue;
    }

    let schedule = match cron::Schedule::from_str(cron_expr) {
      Ok(s) => s,
      Err(e) => {
        tracing::warn!("Invalid cron expression for deployment {}: {e}", deployment.id);
        continue;
      }
    };

    // Check if the schedule should fire now (within the last 60 seconds)
    let last_fire = schedule.prev(&now).unwrap_or(&now);
    let diff = now.signed_duration_since(*last_fire).num_seconds();
    if diff < 60 {
      // Fire backup
      if let Err(e) = super::backup_deployment_volumes(&deployment.id).await {
        tracing::warn!("Scheduled backup failed for deployment {}: {e}", deployment.id);
      }
    }
  }

  // Same for stacks
  let stacks: Vec<komodo_client::entities::stack::Stack> = find_collect(
    &db_client().stacks,
    doc! {},
    None,
  )
  .await?;

  for stack in stacks {
    let Some(backup_config) = &stack.config.backup else {
      continue;
    };
    let Some(cron_expr) = &backup_config.schedule else {
      continue;
    };

    if stack.info.migration_state.is_some() {
      continue;
    }

    let schedule = match cron::Schedule::from_str(cron_expr) {
      Ok(s) => s,
      Err(e) => {
        tracing::warn!("Invalid cron expression for stack {}: {e}", stack.id);
        continue;
      }
    };

    let last_fire = schedule.prev(&now).unwrap_or(&now);
    let diff = now.signed_duration_since(*last_fire).num_seconds();
    if diff < 60 {
      if let Err(e) = super::backup_stack_volumes(&stack.id).await {
        tracing::warn!("Scheduled backup failed for stack {}: {e}", stack.id);
      }
    }
  }

  Ok(())
}
```

- [ ] **Step 5: Register module and spawn scheduler**

In `bin/core/src/main.rs`, add to module declarations:

```rust
mod backup;
```

In the `app()` function (around line 30-90), after `state::init_db_client().await`, add:

```rust
tokio::spawn(backup::scheduler::run_scheduler());
```

- [ ] **Step 6: Verify compilation**

Run: `CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp cargo check --workspace 2>&1 | tail -30`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: add Core backup operations and cron-driven scheduler

backup_deployment_volumes / backup_stack_volumes functions handle
on-demand and scheduled volume backups via the Periphery RPCs.
Scheduler ticks every 60s, fires backups matching cron schedules,
skips deployments in migration state. Retention enforcement
deletes oldest backups beyond max_backups."
```

---

### Task 8: Server Drain State and Migration Orchestration

**Covers:** [S8], [S7] (migration sequence part)

**Files:**
- Modify: `client/core/rs/src/entities/server.rs` — add `Draining`/`Drained` to `ServerState`; add `desired_state`/`drain_timeout_seconds` to `ServerConfig`; add `ServerDesiredState` enum
- Create: `bin/core/src/server/drain.rs` — drain controller + migration orchestration
- Modify: `bin/core/src/main.rs` — add `mod server;` and spawn drain controller
- Modify: `bin/core/src/resource/server.rs` — handle `desired_state` changes in `post_update`
- Modify: `bin/core/src/api/execute/` — add `DrainServer`, `CancelDrain`, `GetDrainStatus` endpoints (if an execute API exists)

**Interfaces:**
- Consumes:
  - `pick_target` from `bin/core/src/placement/mod.rs` (Task 4)
  - `backup_deployment_volumes` from `bin/core/src/backup/mod.rs` (Task 7)
  - `RestoreVolume`, `ReadContainerPorts` RPCs from Tasks 6/3
  - `RemoveContainer` existing RPC
  - Server/Deployment/Stack entities
- Produces:
  - `pub async fn run_drain_controller()` — background task
  - `pub async fn migrate_deployment(deployment_id: &str, target_server_id: Option<&str>) -> anyhow::Result<()>` — single migration
  - `ServerConfig.desired_state: ServerDesiredState` (`Run`/`Drain`)
  - `ServerState::Draining` / `ServerState::Drained`
  - API endpoints: `DrainServer`, `CancelDrain`, `GetDrainStatus`

- [ ] **Step 1: Add drain state to ServerState and ServerConfig**

In `client/core/rs/src/entities/server.rs`:

Add to the `ServerState` enum (line 448):

```rust
/// Server is being drained — deployments are being migrated off.
Draining,
/// Server has been fully drained — no deployments remain.
Drained,
```

Add the `ServerDesiredState` enum (near the bottom of server.rs):

```rust
#[typeshare]
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum ServerDesiredState {
  #[default]
  Run,
  Drain,
}
```

Add to `ServerConfig` (line 105):

```rust
pub desired_state: ServerDesiredState,
pub drain_timeout_seconds: u64,
```

Update the `Default` impl for `ServerConfig` — `desired_state` defaults to `Run`, `drain_timeout_seconds` defaults to `1800`.

- [ ] **Step 2: Create drain controller**

Create `bin/core/src/server/drain.rs`:

```rust
use std::collections::HashMap;
use anyhow::Context;
use komodo_client::entities::{
  deployment::{Deployment, MigrationState},
  resource::Resource,
  server::{Server, ServerDesiredState, ServerState},
  stack::Stack,
};
use mungos::find::find_collect;
use crate::{
  backup::backup_deployment_volumes,
  config::backup_destination,
  helpers::periphery_client,
  placement,
  state::db_client,
};

/// Background task that reconciles server drain states.
pub async fn run_drain_controller() {
  loop {
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    if let Err(e) = tick().await {
      tracing::warn!("Drain controller tick failed: {e}");
    }
  }
}

async fn tick() -> anyhow::Result<()> {
  let servers: Vec<Server> = find_collect(&db_client().servers, doc! {}, None).await?;

  for server in &servers {
    if server.config.desired_state == ServerDesiredState::Drain
      && server.info.state != ServerState::Drained
    {
      // Transition to Draining if not already
      if server.info.state != ServerState::Draining {
        db_client()
          .servers
          .update_one_by_id(
            &server.id,
            mungos::update::Update::Set(doc! { "info.state": "Draining" }),
            None,
          )
          .await?;
      }

      // Find deployments on this server that are idle (not migrating)
      let deployments: Vec<Deployment> = find_collect(
        &db_client().deployments,
        doc! { "info.assigned_server": &server.id, "info.migration_state": null },
        None,
      )
      .await?;

      if deployments.is_empty() {
        // Also check stacks
        let stacks: Vec<Stack> = find_collect(
          &db_client().stacks,
          doc! { "info.assigned_server": &server.id, "info.migration_state": null },
          None,
        )
        .await?;

        if stacks.is_empty() {
          // No deployments or stacks left — transition to Drained
          db_client()
            .servers
            .update_one_by_id(
              &server.id,
              mungos::update::Update::Set(doc! { "info.state": "Drained" }),
              None,
            )
            .await?;
          continue;
        }

        // Migrate the first idle stack (serial per source server)
        if let Err(e) = migrate_stack(&stacks[0].id, None).await {
          tracing::warn!("Stack migration failed for {}: {e}", stacks[0].id);
          mark_stack_failed(&stacks[0].id, &e.to_string()).await;
        }
        continue;
      }

      // Migrate the first idle deployment (serial per source server)
      if let Err(e) = migrate_deployment(&deployments[0].id, None).await {
        tracing::warn!("Deployment migration failed for {}: {e}", deployments[0].id);
        mark_deployment_failed(&deployments[0].id, &e.to_string()).await;
      }
    }
  }

  Ok(())
}

/// Migrate a single deployment from its current server to a new target.
pub async fn migrate_deployment(
  deployment_id: &str,
  target_server_id: Option<&str>,
) -> anyhow::Result<()> {
  let deployment: Deployment = db_client()
    .deployments
    .find_one(doc! { "_id": deployment_id })
    .await?
    .context("Deployment not found")?;

  let source_server_id = deployment.info.assigned_server.clone();

  // Step 1: Mark as migrating
  db_client()
    .deployments
    .update_one_by_id(
      deployment_id,
      mungos::update::Update::Set(doc! {
        "info.migration_state": { "type": "Migrating", "params": {
          "target_server_id": target_server_id.unwrap_or(""),
          "started_at": chrono::Utc::now().timestamp()
        }}
      }),
      None,
    )
    .await?;

  // Step 2: Backup volumes on source
  if !deployment.config.volumes.is_empty() {
    backup_deployment_volumes(deployment_id)
      .await
      .context("Backup failed during migration")?;
  }

  // Step 3: Pick target
  let hint = target_server_id.unwrap_or("");
  let target_id = placement::pick_target(&deployment.config.ports, hint)
    .await
    .context("Failed to pick target for migration")?;

  let target_server: Server = db_client()
    .servers
    .find_one(doc! { "_id": &target_id })
    .await?
    .context("Target server not found")?;
  let target_periphery = periphery_client(&target_server).await?;

  // Step 4: Restore volumes on target
  let dest = backup_destination().context("Backup destination not configured")?;
  for vm in &deployment.config.volumes {
    // Get latest backup key from info.last_backup
    let last_backup = deployment.info.last_backup.get(&vm.volume)
      .context(format!("No backup found for volume {}", vm.volume))?;

    target_periphery
      .request(periphery_client::api::volume_backup::RestoreVolume {
        deployment_id: deployment_id.to_string(),
        volume_name: vm.volume.clone(),
        source_key: last_backup.s3_key.clone(),
        destination: dest.clone(),
      })
      .await
      .context("RestoreVolume RPC failed")?;
  }

  // Step 5: Deploy on target (using existing DeployContainer flow)
  // This involves calling the existing deployment execution path.
  // For now, we update the deployment's server_id and trigger a redeploy.
  db_client()
    .deployments
    .update_one_by_id(
      deployment_id,
      mungos::update::Update::Set(doc! {
        "config.server_id": &target_id,
      }),
      None,
    )
    .await?;

  // Trigger redeploy — this calls the existing DeployContainer execute API
  // The exact mechanism depends on Komodo's execute API structure.
  // TODO: call the existing deploy execution path.

  // Step 6: Read back ports on target
  let ports_response = target_periphery
    .request(periphery_client::api::placement::ReadContainerPorts {
      container_name: deployment.name.clone(),
    })
    .await
    .context("ReadContainerPorts RPC failed")?;

  // Step 7: Stop on source
  let source_server: Server = db_client()
    .servers
    .find_one(doc! { "_id": &source_server_id })
    .await?
    .context("Source server not found")?;
  let source_periphery = periphery_client(&source_server).await?;
  source_periphery
    .request(periphery_client::api::container::RemoveContainer {
      name: deployment.name.clone(),
      signal: deployment.config.termination_signal.into(),
      time: deployment.config.termination_timeout.into(),
    })
    .await
    .context("Failed to remove container on source")?;

  // Step 8: Commit — set assigned_server and clear migration state
  db_client()
    .deployments
    .update_one_by_id(
      deployment_id,
      mungos::update::Update::Set(doc! {
        "info.assigned_server": &target_id,
        "info.host_ports": mungos::serialize(&ports_response.ports)?,
        "info.migration_state": null,
      }),
      None,
    )
    .await?;

  Ok(())
}

/// Migrate a stack. Similar to migrate_deployment but for stacks.
pub async fn migrate_stack(stack_id: &str, target_server_id: Option<&str>) -> anyhow::Result<()> {
  // Analogous to migrate_deployment but uses ComposeUp instead of DeployContainer.
  // Implementation follows the same 8-step pattern.
  // For brevity in this plan, the structure mirrors migrate_deployment.
  todo!("Stack migration — same pattern as migrate_deployment")
}

async fn mark_deployment_failed(deployment_id: &str, reason: &str) {
  let _ = db_client()
    .deployments
    .update_one_by_id(
      deployment_id,
      mungos::update::Update::Set(doc! {
        "info.migration_state": { "type": "Failed", "params": {
          "reason": reason,
          "at": chrono::Utc::now().timestamp()
        }}
      }),
      None,
    )
    .await;
}

async fn mark_stack_failed(stack_id: &str, reason: &str) {
  let _ = db_client()
    .stacks
    .update_one_by_id(
      stack_id,
      mungos::update::Update::Set(doc! {
        "info.migration_state": { "type": "Failed", "params": {
          "reason": reason,
          "at": chrono::Utc::now().timestamp()
        }}
      }),
      None,
    )
    .await;
}
```

- [ ] **Step 3: Create server module and register**

Create `bin/core/src/server/mod.rs`:

```rust
pub mod drain;
```

In `bin/core/src/main.rs`, add to module declarations:

```rust
mod server;
```

In the `app()` function, after `state::init_db_client().await`, add:

```rust
tokio::spawn(server::drain::run_drain_controller());
```

- [ ] **Step 4: Handle desired_state changes in Server resource**

In `bin/core/src/resource/server.rs`, in `post_update` (or equivalent), add logic to handle `desired_state` transitions:

```rust
if updated.config.desired_state == ServerDesiredState::Drain {
  // The drain controller will pick this up on its next tick (every 10s)
  // and transition state to Draining.
  tracing::info!("Server {} marked for drain", updated.id);
} else if updated.config.desired_state == ServerDesiredState::Run {
  // Cancel drain — transition back to Ok/NotOk based on health
  // The drain controller will handle this on next tick.
  tracing::info!("Server {} drain cancelled", updated.id);
}
```

Import `ServerDesiredState` from the entities crate.

- [ ] **Step 5: Add API endpoints (DrainServer, CancelDrain, GetDrainStatus)**

This step depends on Komodo's execute API structure. The execute API is in `bin/core/src/api/execute/`. Follow the existing pattern for execute endpoints — add a new file `bin/core/src/api/execute/server.rs` (or extend the existing server execute file if one exists).

For `DrainServer { server_id }`: update `ServerConfig.desired_state = Drain` via the resource update flow.
For `CancelDrain { server_id }`: update `ServerConfig.desired_state = Run`.
For `GetDrainStatus { server_id }`: query deployments with `assigned_server == server_id` and return counts.

These are thin wrappers over the resource update and read APIs. The exact execute endpoint structure should mirror existing endpoints like `DeployContainer` or `ComposeUp`.

- [ ] **Step 6: Verify compilation**

Run: `CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp cargo check --workspace 2>&1 | tail -30`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: add node draining, migration orchestration, and server drain state

ServerState gains Draining/Drained variants. ServerConfig gains
desired_state (Run/Drain) and drain_timeout_seconds. Drain controller
runs as a background task, migrating deployments off draining servers
using the backup→restore→deploy→stop sequence. Serial per-source-server
migration. API endpoints: DrainServer, CancelDrain, GetDrainStatus."
```

---

## Self-Review Notes

### Spec coverage check

| Spec Section | Covered by Task(s) |
|---|---|
| [S1] Problem | (Context, no implementation needed) |
| [S2] Solution Overview | (Context, no implementation needed) |
| [S3] Scope | (Context, no implementation needed) |
| [S4] Resource Model Changes | Task 1 (Swarm removal), Task 2 (typed ports/volumes + new types) |
| [S5] Placement Scheduler | Task 3 (probe + readback RPCs), Task 4 (algorithm + lifecycle hooks) |
| [S6] Volume Lifecycle | Task 2 (VolumeMount type + validation by typing), Task 5 (compose YAML validation), Task 6 (export/import/retention RPCs) |
| [S7] Backup & Restore Triggers | Task 7 (on-demand + scheduled backup), Task 8 (migration sequence) |
| [S8] Node Draining | Task 8 (drain controller + state machine) |
| [S9] Testing Strategy | Tests included in Tasks 3, 5, 6; full integration tests deferred |
| [S10] Open Questions | (No implementation needed) |

### Type consistency check

- `PortMapping { container: u16, host: Option<u16> }` — defined Task 2, used Task 3 (no, Task 3 uses `AssignedPort`), Task 4 (`pick_target` takes `&[PortMapping]`), Task 8 (migration reads `config.ports`). ✓
- `VolumeMount { volume: String, mount_path: String }` — defined Task 2, used Task 6 (volume discovery via `config.volumes`), Task 7 (backup iterates `config.volumes`), Task 8 (restore iterates `config.volumes`). ✓
- `BackupConfig { schedule: Option<String>, max_backups: u32 }` — defined Task 2, used Task 7 (scheduler reads `config.backup.schedule`, `config.backup.max_backups`). ✓
- `AssignedPort { container: u16, host: u16 }` — defined Task 2, used Task 3 (`ReadContainerPortsResponse.ports`), Task 8 (readback after deploy). ✓
- `BackupDestination { endpoint, region, bucket, access_key, secret_key }` — defined Task 6, used Task 7 (config accessor returns it), Task 8 (migration passes it to `RestoreVolume`). ✓
- `MigrationState` enum — defined Task 2, used Task 8 (set/cleared during migration). ✓
- `ServerDesiredState` enum — defined Task 8, used Task 8 (drain controller checks `config.desired_state`). ✓

### Known gaps

1. **Stack migration** (`migrate_stack`) is marked `todo!()` in Task 8. This should be implemented following the same pattern as `migrate_deployment` but using `ComposeUp` instead of `DeployContainer`. This is a real gap that needs filling during implementation.
2. **Deploy-on-target step** in Task 8 (Step 5 of the migration sequence) references "the existing DeployContainer execute API" but doesn't show the exact call. The implementer should look at `bin/core/src/api/execute/` for the existing deploy execution path and call it.
3. **Full integration tests** (2-node minicluster drain test) are not included in this plan — they require a running Podman + MongoDB + multi-node setup. The plan includes unit tests and smoke tests only.
