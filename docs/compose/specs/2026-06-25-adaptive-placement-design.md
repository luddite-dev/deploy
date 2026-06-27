# Adaptive Placement and Volume Migration Design

Builds on the Komodo fork (`moghtech/komodo`, GPL-V3). Adds three interlocking
capabilities — placement scheduler, S3-backed volume lifecycle, and node
draining — to abstract away the need for operators to manually choose which
server runs each deployment. Iroh transport swap is
explicitly out of scope; this milestone preserves Komodo's existing WebSocket
transport.

Two prior findings motivate this design:

1. Komodo's transport layer is cleanly abstracted behind the `Websocket` trait
   (`lib/transport/src/websocket/mod.rs:40-61`) — Iroh integration is a later
   ~200-300 LOC bolt-on, independent of this milestone.
2. Komodo has no port allocation logic anywhere today (only user-specified port
   strings or Docker's own random ports), no volume backup/export, and only
   database-metadata backups (`lib/database/src/utils/backup.rs:20`) — not volume
   data. This milestone adds both capabilities.

## [S1] Problem

Komodo requires users to set `DeploymentConfig.server_id` (or `swarm_id`) before
a deployment can be created (`bin/core/src/resource/deployment.rs:210-217,
292-294, 457-459`). Operators must manually decide which Periphery node runs
each workload. There is no scheduler. There is no way to migrate deployments
between nodes with their volume data intact: `volumes` is a single free-text
`String` (`client/core/rs/src/entities/deployment.rs:249`) parsed at deploy time
into `Conversion { local, container }` pairs (`deployment.rs:415-431`), and the
only volume operations Komodo supports are list/inspect via bollard and
delete/prune via Docker CLI. Bind mounts and named volumes are not
distinguished at the type level.

This milestone solves two coupled problems on top of the fork:

- **Automatic placement and port allocation** — deployments need to land on a
  node whose free host ports satisfy the deployment's fixed-port requirements,
  HTTP-proxied services need any free high port, and the assigned bindings need
  to be recorded for later Caddy integration.
- **Volume migration for node draining** — when a node needs to leave the
  fleet, its deployments' named-volume data must be exportable to S3-compatible
  storage and restorable on a different node before the new container starts.

These together enable drift-free node decommissioning.

## [S2] Solution Overview

Introduce three Core-side subsystems, each with a thin Periphery RPC extension:

1. **Placement scheduler** (`bin/core/src/placement/`). On Deployment/Stack
   create or update, if no `server_id` hint is set the scheduler picks a target
   by probing candidate Peripheries' free host ports via a new
   `CheckHostPorts` RPC implemented with the `netstat2` crate (`/proc/net/tcp`
   reads; no shell-out to `ss`). The effective target lands in
   `info.assigned_server`. After deploy, a new `ReadContainerPorts` RPC reads
   back the actual host port bindings (set by Podman for container-only
   ports) into `info.host_ports`.
2. **Volume lifecycle & S3 pipeline**. The `volumes` config field is
   retyped to a structured `Vec<VolumeMount> { volume, mount_path }` for
   Deployments and validated against bind mounts in Stack compose files.
   Periphery gains `BackupVolume`, `RestoreVolume`, `ListVolumeBackups` RPCs
   driven by `podman volume export` / `podman volume import` plus the `rust-s3`
   crate. The destination S3 config lives globally in Core and is forwarded
   per-operation to keep Periphery stateless about backup targets.
3. **Drain controller** (`bin/core/src/server/drain.rs`). Adds `Draining` and
   `Drained` to the existing `ServerState` enum, plus a new
   `desired_state: ServerDesiredState` (`Run`/`Drain`) on `ServerConfig`.
   When the operator sets `desired_state = Drain`, Core walks the server's
   deployments, triggering per-deployment migrations (backup → restore on
   target → deploy → readback → stop source) per the [S7] flow.

Build order: subsystems 1 and 2 are orthogonal and may be developed in
parallel; subsystem 3 depends on both.

## [S3] Scope

In scope:

- Dropping Swarm mode entirely (`swarm_id` field, `bin/periphery/src/api/swarm/`
  module, `docker stack` executor, Swarm-only entity types).
- Restructuring `ports` and `volumes` on `DeploymentConfig` and `StackConfig`
  from free-text `String` into typed `Vec<PortMapping>` / `Vec<VolumeMount>`.
- New Core module `bin/core/src/placement/` implementing `pick_target`.
- New Periphery RPCs: `CheckHostPorts`, `ReadContainerPorts`,
  `BackupVolume`, `RestoreVolume`, `ListVolumeBackups`.
- New `bin/core/src/backup/scheduler.rs` module for cron-driven periodic
  backups gated on `config.backup.schedule`.
- New `bin/core/src/server/drain.rs` controller implementing the drain state
  machine and migration orchestration.
- New `ServerConfig.desired_state` (`Run`/`Drain`) and `ServerState::Draining`/
  `Drained` variants.
- Default behavior: on-demand backup before any migration; opt-in periodic
  backups per deployment via `config.backup.schedule`.

Out of scope:

- Iroh transport swap (current WebSocket transport stays).
- Caddy reverse-proxy integration (only the data contract: scheduler records
  `info.host_ports` so a future Caddy controller can read it).
- Backup retention beyond a simple max-backups-per-volume count.
- Multi-region / cross-bucket S3 replication.
- Parallel migration throughput optimizations (this milestone migrates
  per-source-server serially).
- Automated target-health verification beyond "did `docker run` exit 0".

## [S4] Resource Model Changes

All changes are against the existing Komodo entities in
`client/core/rs/src/entities/` and `bin/core/src/resource/`. No backward
compatibility constraints — this is a hard fork.

### DeploymentConfig (deployment.rs:89)

- Drop `swarm_id` (line 101).
- `server_id` (line 114) becomes optional by convention: empty string means
  "scheduler decides"; non-empty means "pin to this server (hint honored when
  feasible, error if unavailable)". Drop the `get_swarm_or_server` dual
  resolution in `bin/core/src/resource/deployment.rs:75-78, 277-294,
  389-411`; replace with plain `server_id`-only resolution.
- Replace `ports: String` (line 239) with:

  ```rust
  pub struct PortMapping {
      pub container: u16,
      pub host: Option<u16>,
  }
  pub ports: Vec<PortMapping>,
  ```

  `host: None` is the HTTP-proxied convention — scheduler picks any node,
  Podman assigns a random high port; the assigned port is read back into
  `info.host_ports` for future Caddy consumption.

- Replace `volumes: String` (line 249) with:

  ```rust
  pub struct VolumeMount {
      pub volume: String,
      pub mount_path: String,
  }
  pub volumes: Vec<VolumeMount>,
  ```

  Bind mounts are unrepresentable by typing — there is no host-path field.

- Add:

  ```rust
  pub struct BackupConfig {
      pub schedule: Option<String>,
      pub max_backups: u32,
  }
  pub backup: Option<BackupConfig>,
  ```

  `schedule = None` (default) means on-demand only. When `Some(cron_expr)`,
  the Core backup scheduler backs up all volumes on the cron tick.

### DeploymentInfo (deployment.rs:69)

Add:

```rust
pub struct AssignedPort {
    pub container: u16,
    pub host: u16,
}

pub assigned_server: String,
pub host_ports: Vec<AssignedPort>,
pub last_backup: HashMap<String, VolumeBackupRecord>,
pub migration_state: Option<MigrationState>,
```

`MigrationState` is `Migrating { target_server_id, started_at }` | `Failed {
reason, at }`. `Idle` is represented by `None`.

### StackConfig (stack.rs:302)

- Drop `swarm_id` (line 314).
- `server_id` (line 327) becomes optional by the same convention as
  DeploymentConfig.
- Stack ports and volumes live inside the compose YAML (`file_contents`
  at line 637), so they are not restructured at the config level. Instead, add
  **validation** that walks each service's `volumes:` list, rejecting any
  host-path source, and rejects Swarm-only compose keys (`deploy:`,
  `replicas`, etc.).
- Add `backup: Option<BackupConfig>` (same struct as DeploymentConfig).

### StackInfo (stack.rs:245)

Add `assigned_server: String`,
`host_ports: HashMap<String, Vec<AssignedPort>>` (keyed by compose service
name), `last_backup: HashMap<String, VolumeBackupRecord>`, and
`migration_state: Option<MigrationState>` (per-stack, not per-service — a Stack
migrates as a single unit).

### Server (server.rs)

- Add `Draining` and `Drained` variants to the existing `ServerState` enum
  (line 448).
- Add `desired_state: ServerDesiredState` to `ServerConfig` (line 105):

  ```rust
  pub enum ServerDesiredState {
      Run,
      Drain,
  }
  ```

  Default is `Run`. Setting it to `Drain` is the operator's request; the drain
  controller reconciles `info.state` from `Ok` → `Draining` → `Drained`.

- Add `drain_timeout_seconds: u64` to `ServerConfig` (default 1800).

### Cleanup (mechanical)

- Delete `bin/periphery/src/api/swarm/` entirely.
- Delete Swarm-mode-only entity types (`Service`, `Swarm`, `Secret`,
  `Config`, etc.) — on the chopping block if they have no non-Swarm use.
- Delete Swarm resource handlers in `bin/core/src/resource/` and any
  `swarm_id` validation/precedence logic.

## [S5] Placement Scheduler

New module `bin/core/src/placement/mod.rs`:

```rust
pub async fn pick_target(
    config: &DeploymentConfig,  // or StackConfig via a trait
    hint_server_id: &str,
    fixed_ports: &[u16],
) -> Result<String, PlacementError>;
```

Invoked from `KomodoResource::validate_create_config` /
`validate_update_config` so a no-eligible-server outcome fails the
create/update cleanly (resource never enters a broken state). The chosen
server_id is written into `info.assigned_server` (NOT into `config.server_id`
— preserves the user's empty-or-hint intent so future reevaluations start
from the user's expression, not the system's last resolution).

### Algorithm

1. Compute `fixed_ports` from `config.ports.iter().filter_map(|p| p.host)`.
2. Candidate servers = all with `info.state == Ok` (excluding `NotOk`,
   `Disabled`, `Draining`, `Drained`).
3. If `config.server_id` (hint) is non-empty and the hinted server is a valid
   candidate: probe its fixed ports. If all free → use it. If not → fail with
   `PlacementError::HintedServerUnavailable`. Do not silently fall back;
   preserves user intent.
4. Otherwise probe each candidate's fixed ports; pick the first with all
   free, preferring the server with the fewest currently-assigned deployments
   (cheap spread heuristic via existing `assigned_server` index lookup).
5. If no candidate has all ports free → fail with
   `PlacementError::NoEligibleServer`.

### Port availability probe (Periphery side)

New RPC `CheckHostPorts { ports: Vec<u16> } -> Vec<u16>` returns the subset
that are free. Implementation: use the `netstat2` crate
(`netstat2::get_sockets_info(AddressFamilyFlags::all(), ProtocolFlags::TCP)`),
filter for `TcpState::Listen`, exclude source ports bound on any local
interface from the response set. Catches all system listeners including
Podman-bound ports and non-Podman processes (e.g. sshd on port 22).

Periphery runs on host (not containerized), so `netstat2` reads the host's
`/proc/net/tcp` directly with no caveats.

### Assigned-port readback (Periphery side)

New RPC `ReadContainerPorts { target } -> Vec<AssignedPort>` (Deployment) or
`HashMap<String, Vec<AssignedPort>>` (Stack). Implementation:

- Deployment: `podman inspect --format json <name>`, extract
  `NetworkSettings.Ports` and map each binding to `AssignedPort`.
- Stack: `docker compose -p <project> ps --format json`, extract per-service
  port bindings into the keyed map.

Core writes the result into `info.host_ports`. This is the data contract
future Caddy integration reads.

### Lifecycle hooks

On create: probe → pick → set `info.assigned_server` → deploy → readback →
set `info.host_ports`.

On update touching ports or volumes: re-evaluate placement (call
`pick_target` with current hint). If the new config still fits on the
current assigned server, stay. If not, hand off to the migration flow in
[S7] (the actual migration mechanics; placement only decides the
target).

On server marked `Drain`: does not call `pick_target` directly; the drain
controller ([S8]) calls `pick_target` per-deployment with empty hint to
find a new home.

## [S6] Volume Lifecycle

### Validation (named-volumes-only enforcement)

- `DeploymentConfig.volumes: Vec<VolumeMount>` is enforced by typing —
  `VolumeMount { volume, mount_path }` has no host-path field.
- `StackConfig.file_contents` (compose YAML) is validated by parsing the
  YAML with `serde_yaml` (already a Komodo dependency), walking each
  service's `volumes:` list and rejecting any source that is not a named
  volume declared in the top-level `volumes:` map. Also reject Swarm-only
  compose keys (`deploy:`, `replicas`, `placement`).

Startup-time Periphery probe: `podman volume export --help` and
`podman volume import --help` must succeed, else Periphery refuses to start
with a clear "unsupported Podman version" error. No fall back path.

### S3 client (Periphery-side)

Add the `rust-s3` crate. The S3 destination config (`BackupDestination {
endpoint, region, bucket, access_key, secret_key }`) lives in **Core** config
(global, one bucket for all deployments). Core forwards the destination to
Periphery on each backup/restore operation; the request payload carries
`destination: BackupDestination`. Periphery stays stateless about backup
targets and never reads a per-Periphery S3 config.

S3 key layout: `backups/deployments/<deployment_id>/volumes/<volume_name>/
<timestamp>.tar`. The "latest" snapshot is the highest-timestamped key for
that volume under the prefix. For Stacks, the same layout applies with
`<stack_id>` substituted for `<deployment_id>` and each compose named
volume getting its own subprefix.

### Volume export (Periphery side)

`BackupVolume { deployment_id, volume_name, destination } -> BackupResult {
s3_key, size_bytes, checksum }`. Steps:
- `podman volume export <name> --output /tmp/<name>-<timestamp>.tar`.
- Upload tarball to S3 under the layout above.
- Delete local temp file.
- Return `BackupResult` (checksum used to detect partial uploads on retry).

### Volume import (Periphery side)

`RestoreVolume { deployment_id, volume_name, source_key, destination } ->
RestoreResult { bytes_restored }`. Steps:
- Download tarball from S3 to `/tmp/<name>-<timestamp>.tar`.
- `podman volume create <name>` (idempotent — succeeds if already exists;
  restore overwrites contents).
- `podman volume import <name> /tmp/<name>-<timestamp>.tar`.
- Delete local temp file.

### Volume discovery

- Deployment: volumes come directly from `config.volumes: Vec<VolumeMount>`.
- Stack: parse the compose YAML's top-level `volumes:` section; the named
  keys are the volumes to back up. Each named volume is what Podman will
  materialize as `<project>_<name>` on disk; backups use the logical name
  and the migration flow includes the project prefix when issuing
  `BackupVolume` / `RestoreVolume`.

### Shared backup record types

```rust
pub struct VolumeBackupInfo {
    pub s3_key: String,
    pub timestamp: i64,
    pub size_bytes: u64,
}

pub struct VolumeBackupRecord {
    pub s3_key: String,
    pub timestamp: i64,
    pub size_bytes: u64,
    pub checksum: String,
}
```

`VolumeBackupInfo` is the listing entry; `VolumeBackupRecord` adds `checksum`
and is what Core persists per-volume in `info.last_backup` for retry
verification and checksum validation on restore.

### Periphery operations summary

- `CheckHostPorts { ports: Vec<u16> } -> Vec<u16>` ([S5]).
- `ReadContainerPorts { target } -> Vec<AssignedPort>` or
  `HashMap<String, Vec<AssignedPort>>` ([S5]).
- `BackupVolume { deployment_id, volume_name, destination } -> BackupResult`.
- `RestoreVolume { deployment_id, volume_name, source_key, destination } ->
  RestoreResult`.
- `ListVolumeBackups { deployment_id, volume_name, destination } ->
  Vec<VolumeBackupInfo>` (used by Core for retention enforcement).

### Retention (`max_backups`)

After every successful backup (both scheduled and on-demand),
`ListVolumeBackups` returns the prior backups for that volume sorted by
timestamp. Core (or Periphery during the backup op) deletes S3 objects beyond
`config.backup.max_backups` oldest-first. Default value `7`. No tiered
retention.

## [S7] Backup & Restore Triggers

### On-demand backup (Core action)

New Core API operation `BackupDeploymentVolumes { deployment_id }` (and the
Stack equivalent). For each volume in `config.volumes` (or Stack YAML),
calls Periphery `BackupVolume`. Updates `info.last_backup` per volume
including `s3_key`, `size_bytes`, `timestamp`, `checksum`.

### Scheduled backups

New `bin/core/src/backup/scheduler.rs` module. Tick behavior:
- Enumerate all Deployments and Stacks with
  `config.backup.schedule: Some(cron_expr)`.
- Parse the cron expression via the `cron` crate (lightweight, pure parser).
- On tick, fire `BackupDeploymentVolumes` / equivalent for the deployment.
- Skip deployments currently in `info.migration_state == Migrating` or
  `Failed` to avoid compounding failures.
- After each successful backup, enforce `max_backups` retention per [S6].

The scheduler is a single Core background task that wakes on cron next-fire
times; it does not embed a full cron daemon.

### Migration sequence

Triggered by the drain controller ([S8]) or by an explicit operator
`MigrateDeployment { deployment_id, target_server_id: Option<String> }`
action. Steps, with rollback semantics:

1. **Backup on source** — call `BackupDeploymentVolumes(D)` on the source
   Periphery. Failure → abort migration, mark
   `info.migration_state = Failed { reason: "backup_failed", at: now }`,
   alert operator, leave deployment on source.
2. **Pick target** — call `pick_target(&config, hint, &fixed_ports)` where
   `hint` is `target_server_id` if the operator passed one, else empty.
   Source is excluded from candidates because its `state == Draining`. No
   eligible target → mark `Failed { reason: "no_target" }`, leave on source.
3. **Restore on target** — for each volume in `config.volumes`, call
   `RestoreVolume(D, volume_name, latest_backup_key, destination)` on the
   target Periphery. Volume is created and populated on target before any
   deploy. Failure at the i-th volume → roll back volumes 1..i-1 (the
   already-restored ones) by calling `DeleteVolume` for each; mark
   `Failed { reason: "restore_failed" }`; leave on source.
4. **Deploy on target** — issue the existing `docker run` / `docker compose
   up` flow against the target. Uses the freshly-restored volumes. Failure →
   same rollback as step 3 (DeleteVolume for each restored volume); mark
   `Failed { reason: "deploy_failed" }`; leave on source.
5. **Readback ports on target** — call `ReadContainerPorts` on the target
   container. Update `info.host_ports`.
6. **Stop on source** — `RemoveContainer` on source. Source named volumes
   are left behind (operator can prune via Komodo's existing
   `PruneVolumes` op if desired). Failure here is the only non-rollback-safe
   step: if target is healthy but source can't be removed, elevate to
   operator alert. Source already knows its `state == Draining` so no new
   placements target it.
7. **Commit** — set `info.assigned_server = target`,
   `info.migration_state = None`. Per-deployment migration complete.

### Concurrency within a single migration

Steps 1-6 run serially per deployment. Step 3 (volume restore) cannot be
parallelized within a single deployment because order matters for rollback
traceability; parallelizing across multiple migrations of different
deployments is governed by [S8]'s per-source-server serial rule.

## [S8] Node Draining

### Drain controller

New `bin/core/src/server/drain.rs`. Core reconcile loop checks
`Server.config.desired_state == Drain && info.state != Drained` for each
server. If true and `info.state` is `Ok` or `NotOk`, transition
`info.state = Draining`. When `Draining`:

For each deployment D with `info.assigned_server == this_server` and
`info.migration_state == Idle`:
  - Begin a migration per [S7] flow (pick target with empty hint so the
    scheduler is free to choose; this_server excluded as a candidate).
  - Serialize per source server: only one migration in flight at a time per
    draining server. Other source servers may drain in parallel — each has
    its own drain-controller invocation.

### Drain completion

`Draining` → `Drained` when both true:

1. No deployments remain with `assigned_server == this_server` (all migrated
   successfully), AND
2. No `Migrating` deployments remain (no in-flight migrations on this
   server).

`MigrationFailed` deployments **block** the `Drained` transition. Operator
resolves each by one of:

- **Retry** — explicit action resets `info.migration_state = None` and the
  drain controller picks it up again.
- **ForceDelete** — operator acknowledges data loss and deletes the
  deployment outright; it no longer blocks drain.
- **Pin to dying server** — set `config.server_id = this_server` to
  explicitly mark "this is fine to die with the node" (the scheduler will
  not migrate it). The deployment remains on the draining server; if the
  server actually dies, the deployment is lost by design.

### Drain timeout

Per-deigration timeout governed by `ServerConfig.drain_timeout_seconds`
(default 1800 = 30 min). On timeout, mark the deployment
`migration_state = Failed { reason: "timeout" }`, move on to the next
deployment in the queue. Does not abort the overall drain.

### Endpoint surface (Core API)

- Set `desired_state` via existing `KomodoResource::update` on the Server
  (config change of `desired_state`).
- `DrainServer { server_id }` — convenience wrapper that performs the
  update with `desired_state = Drain`.
- `CancelDrain { server_id }` — sets `desired_state = Run`. Transitions
  `info.state` back to `Ok` / `NotOk` per actual server health probe.
  In-flight migrations continue to completion; only new migrations are
  suppressed.
- `GetDrainStatus { server_id }` — returns:
  - `total_original_deployments: u32`
  - `migrated: u32`
  - `in_progress: u32`
  - `failed: u32` with per-failure `{ deployment_id, reason }`
  - `target_server_assignment_counts: HashMap<String, u32>` (where work
    was migrated to, for operator visibility).

### New placements to a draining server are blocked

The [S5] placement algorithm's candidate filter excludes servers with
`info.state == Draining`, `Drained`, `Disabled`, or `NotOk`. Operator pinning
(`config.server_id` non-empty) on a draining server is honored only if the
hinted server is `Draining` — explicitly, the only way to land a deployment
on a draining server is to pin it there (the user is saying "I want this
to die with the node").

### Reviving a drained node

Operator sets `desired_state = Run`. Core transitions `info.state = Ok`
(subject to actual health probe). Node re-enters the placement candidate
pool. Already-migrated deployments are not moved back automatically; the
node now accepts new placements based on its free port availability.

## [S9] Testing Strategy

Per-subsystem test boundaries:

- **Placement**: unit test `pick_target` with stubbed `CheckHostPorts` RPC
  responses covering: empty hint flow, hint-with-all-ports-free, hint with
  a busy port fails cleanly, no-eligible-server failure, candidate spread
  heuristic ordering.
- **Volume validation**: unit tests on the Stack compose YAML validator
  — reject bind mounts, reject Swarm-only compose keys, accept valid
  named-volume-only compose files.
- **Volume export/import**: integration test against a local Podman
  install and either MinIO or a stub S3 via `rust-s3`; verify round-trip
  (export then import produces identical file tree).
- **Drain controller**: integration test exercising a 2-node minicluster,
  draining one node and asserting all deployments land on the survivor
  with restored volumes.
- **Migration failure paths**: tests for each rollback step (backup
  fails, restore fails partway, deploy fails, source stop fails) verifying
  algorithmic state left behind.

Existing Komodo test infrastructure (if any is found in the clone) is the
first chance to extend rather than reinvent; checked at planning time.

## [S10] Open Questions

None blocking at design time. Implementation plan will surface concrete
Podman version requirement for `volume export`/`volume import` and decide
the cron expression feature set (standard 5-field vs. extended).
