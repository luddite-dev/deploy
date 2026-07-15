# Forking — Upstream Sync Tracking

This document tracks our ongoing effort to cherry-pick valuable changes from
upstream Komodo (`moghtech/komodo`) into our hard fork (`luddite-dev/deploy`).

It records: what we drop, what we have merged, what we are porting now, and what
we plan to merge later once sufficient progress is made either upstream or on
our end. Commit hashes (full where stable) are referenced throughout.

---

## Fork context

| Item                      | Value                                                       |
| ------------------------- | ----------------------------------------------------------- |
| Upstream remote           | `moghtech/komodo` (remote `komodo`)                         |
| Upstream tip tracked      | `komodo/2.3.0` → `a53b2edbb0fd90c1d3b4b4ca2252a94642737be7` |
| Fork base (merge-base)    | `7c44ffc8eedc8fb08ceb5a178e1e13504f59f619`                  |
| Our canonical branch      | `main` on `github.com/luddite-dev/deploy`                   |
| Swarm drop commit         | `fee066657b21f43bd5484c4de7b5568436e9e9cd`                  |
| Divergence at last review | 89 upstream commits, 55 fork commits                        |

The fork is a **hard fork**: no backward-compatibility constraints, types may be
freely broken, no migration shims. All Komodo types can be rewritten freely.

---

## Drop rules

The following categories of upstream changes are **dropped** and never ported.
When a cherry-pick touches these, the affected hunks are surgically removed and
the remaining, swarm-free hunks are kept.

### Rule 1 — Swarm mode (explicit drop for simplicity)

Swarm mode was removed entirely in commit `fee066657`. We do **not** want Swarm
back. Any upstream commit that touches Swarm-specific files or fields is either
skipped entirely (if purely swarm-oriented) or cherry-picked with the swarm
hunks mechanically stripped:

**Files that no longer exist in our fork** (upstream hunks against these are
dropped):

- `bin/core/src/api/read/swarm.rs` — deleted
- `bin/core/src/api/write/swarm.rs` — deleted
- `client/core/rs/src/entities/docker/{node,secret,service,swarm,task}.rs` —
  deleted
- `bin/core/src/api/read/mod.rs` swarm exports — removed
- `bin/core/src/api/write/mod.rs` swarm exports — removed

**Fields/patterns stripped from upstream diffs when they appear**:

- `swarm_id` fields on config structs (deployments, stacks)
- `swarm_name` lookups in `all_resources_cache()` blocks (`all.srms`, etc.)
- `list_resource_ids_for_user::<Swarm>` calls in `user_resource_target_query`
- `update_swarm_stack_cache` calls in `monitor/resources.rs`
- `<Swarm>` generic instantiations in permission/resource-listing code
- Swarm pagination/filter commits: `52a38f39f` (swarm pagination + cli),
  `023754bbd` (deployment/stack filter by swarm id), `05da009ce` (update
  permissions on swarm) — all skipped entirely

**Rationale:** `moghtech/komodo` ships Swarm as a first-class deployment target
alongside `server_id`. Our fork uses `server_id` as the sole deployment target
field and replaces Swarm-based dispatch with an adaptive placement scheduler
that picks target nodes by probing free host ports. Reintroducing Swarm would
re-add a second dispatch dimension we deliberately removed for simplicity.

### Rule 2 — Commercialization / vendor coupling (explicit drop)

Upstream changes that couple the Core to Mogh-specific commercial services (e.g.
mogh auth server integration beyond what we already vend, mogh-ui login flows
tied to a hosted backend) are dropped unless we explicitly adopt that service.
Version bumps of `mogh_auth_server`, `mogh-ui` are evaluated case by case.

### Rule 3 — UI-only changes with no backend dependency

UI (.tsx/.ts) commits are deferred, not dropped. They require the Rust API layer
to land first; we port UI as a bundle after the Rust tier it depends on.

### Rule 4 — WebSocket/Noise transport changes (explicit drop after M3)

Milestone 3 replaced the entire WebSocket + mutual Noise XX handshake transport
with Iroh (QUIC + TLS 1.3, raw public keys). Upstream commits that touch the old
transport stack are no longer cherry-pickable:

**Files that no longer exist in our fork** (upstream hunks against these are
dropped):

- `lib/transport/src/auth.rs` — deleted (Noise handshake layer)
- `lib/transport/src/websocket/` — deleted (entire WS trait family)
- `lib/transport/src/timeout.rs` — deleted (WS-specific timeout wrapper)
- `bin/core/src/connection/client.rs` — deleted (Core no longer dials out)
- `bin/periphery/src/connection/server.rs` — deleted (Periphery no longer
  listens)
- `bin/periphery/src/helpers.rs` SSL functions — deleted (no self-signed certs)

**Fields/patterns stripped from upstream diffs when they appear**:

- `ServerConfig.address`, `insecure_tls`, `passkey` — deleted (unified
  direction)
- `ServerInfo.public_key` → renamed to `endpoint_id` (Iroh EndpointId)
- `PeripheryConfig.private_key` → renamed to `iroh_secret_key`
- `CoreConfig.periphery_public_keys` → renamed to `iroh_periphery_endpoint_ids`
- `LoginMessage` variants (`Nonce`, `Handshake`, `OnboardingFlow`, `PublicKey`,
  `V1PasskeyFlow`, `V1Passkey`) — deleted, replaced with `OnboardingToken`/
  `EndpointId`/`Success`
- `LoginFlow`, `PublicKeyValidator`, `ConnectionIdentifiers`, `Websocket` trait
  — all deleted

**Rationale:** Iroh's built-in TLS 1.3 + raw public key mutual auth makes the
Noise layer redundant (double-encryption otherwise). The `Websocket` trait is
WS-frame-oriented and would need a length-prefix framing hack for Iroh
byte-streams. Since the hot paths (`handle_socket`, `handle_login`,
`handle_incoming_message`) were rewritten regardless, a full replacement was
cleaner than an adapter.

---

## Merged

_Commits pulled from upstream and landed on `main`._ All three tiers below are
merged via PRs [#8](https://github.com/luddite-dev/deploy/pull/8),
[#9](https://github.com/luddite-dev/deploy/pull/9),
[#10](https://github.com/luddite-dev/deploy/pull/10). Upstream commit hashes are
shown for traceability; on `main` they appear as rebase-merged commits with
rewritten short hashes (e.g. `807c7f4f6`, `93fabc149`, `dd84c84d3`).

<details>
<summary>Tier 1 — clean picks (ZFS ARC memory, startup alerts, error formatting) — <b>MERGED via PR #8</b></summary>

Cherry-picks with no swarm entanglement — immediate value, minimal merge risk.
Branch: `upstream/tier-1-clean-picks` off `main`.

| Upstream commit                            | Summary                                                             | Notes                                                                                                                                                                                                                                                    |
| ------------------------------------------ | ------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ab1b8ca3f8a99370d3a5ff5401d21caa07a080ff` | ZFS ARC / cache-buffers memory breakout in periphery stats          | High value for host-based periphery on ZFS hosts. Reads `/proc/meminfo` + `/proc/spl/kstat/zfs/arcstats`; subtracts ARC from "used" to prevent false high-memory alerts. Applied cleanly (module move `stats.rs`→`stats/mod.rs` resolved automatically). |
| `dded910ea3320a807a8cbe4152a6118ae69f6d54` | `cargo fmt` follow-up to the mem module                             | Companion to `ab1b8ca3f`. Clean.                                                                                                                                                                                                                         |
| `daa5d55f1a1ca81d0314259b5097dbae1d0a0657` | Skip alerting check on initial server cache refresh after startup   | Avoids spurious startup alerts on multi-node refresh. `monitor/mod.rs` matches pre-commit shape — clean pick.                                                                                                                                            |
| `64dde24c5186bf794ae15e5bb55d239e3f2719ef` | Single-line command error formatting (`{e:?}` + `from_err_message`) | Improves log readability across all command-driven actions. Clean.                                                                                                                                                                                       |

> Originally planned: `00f77c249` (container tag filters + case-insensitive
> search). **Moved to Tier 2** — it depends on pagination infrastructure
> (`DEFAULT_LIST_LIMIT`, `Option<U64>`, `self.page`) that only exists after the
> Tier 2 pagination commits land. Attempted as a Tier 1 pick; conflicts in
> `bin/core/src/api/read/{docker.rs,stack.rs}` confirmed the dependency.

</details>

<details>
<summary>Tier 2 — pagination backbone (mechanical swarm surgery) — <b>MERGED via PR #9</b></summary>

High-value performance refactor. Applied as a unit after Tier 1. Each commit
touches `read/swarm.rs` (deleted) or `swarm_name` blocks — stripped per Drop
Rule 1. Branch: `upstream/tier-2-pagination` off the tier 1 branch.

| Upstream commit                            | Summary                                                                                    | Swarm surgery                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| ------------------------------------------ | ------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `47a6dfe95c74b9370fa945643f02cb234882606c` | Startup plumbing for pagination in `permission.rs`                                         | Drop `user_resource_target_query` swarm block + `<Swarm>` query.                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `bb55120d94441de3f703b496956718054def2e98` | Wire `limit`/`page` into every `List*` Resolve + client structs                            | Drop `read/swarm.rs` hunk.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `d8d731cfd57ffdbbef86b72427cdf3f578af2283` | `Option<U64>` limit, `default_list_limit`                                                  | Drop `read/swarm.rs` hunk.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `3117aa26b4aab29c6f0b0ee0676e378f5858fdec` | DB cursor streaming / `ListPermits` / `load_list_permits` — replaces collect-then-filter   | Drop `read/swarm.rs` hunk. Largest single perf win. ~228 lines `permission.rs`/`resource/mod.rs`.                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `ef2b6ebc46ea670e4e0e459608a6a97245239a80` | Fix `list permits` to respect `resource.base_permission` (`perm.elevate(...)`)             | 3-line correctness fix on top of `3117aa26b`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `42367c530b0c458dc7b26a62823b17d05e8a0628` | `saturating_mul` in skip computations                                                      | Overflow hardening, no swarm.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `97ce46d89aaf56f493d705bdb4576421906339b3` | Linked resource names from `all_resources_cache()` (avoids N×M per-item DB lookups)        | Drop `swarm_name` hunks (`all.srms`, `deployment.config.swarm_id`, swarm-stack-cache). Keep `server_name`/`build_name`/`repo_name`/`stack_name` hunks + `StackService.stack_name` field.                                                                                                                                                                                                                                                                                                                                       |
| `00f77c249c17dbeb740f3810b97d0f323930962f` | Container/stack-service tag filters + case-insensitive search terms, `saturating_mul` skip | Moved from Tier 1. Depends on pagination fields (`DEFAULT_LIST_LIMIT`, `Option<U64>`, `page`). Swarm-free but conflicts with our `docker.rs`/`stack.rs` until pagination lands. **Resolution**: upstream renamed `ListAllDockerContainers.containers` → `.terms`; our fork keeps old field name (rename is part of de-vendor Tier 5). Upstream renamed `ListAllDockerContainersResponse` → `ListAllContainersResponse`; our fork keeps old type name (de-vendor Tier 5). Wildcard matching replaced with lowercase `contains`. |

### Tier 2 additional resolutions

- **State filtering deferred**: `3117aa26b` closures reference
  `self.query.specific.states` (added by upstream `8f7854599`, a Tier 4 deferred
  candidate). Closures use `|_| true` for handlers without other filters;
  deployment/stack closures keep `only_update_available` check only.
- **`terms` field name**: upstream renamed `containers`/`services` → `terms` on
  `ListAllDockerContainers`/`ListAllStackServices` clients. Our fork retains the
  old field names; handler code uses `.containers`/`.services`.
- **Type name preservation**: `ListAllDockerContainersResponse` kept (not
  renamed to `ListAllContainersResponse` — that's de-vendor commit `6ca10c9e5`,
  Tier 5 deferred).
- **`list_items_for_user` conflict pattern**: every `ListXxx` handler had the
  same conflict shape —
  `list_for_user(query, limit as i64, self.page * limit, ...)` →
  `list_items_for_user(query, limit, self.page, ..., |item| filter_closure)`.
  Always take upstream side. Redundant post-filter blocks (e.g.
  `only_update_available` in `deployment.rs`) dropped since the closure handles
  filtering inline.

</details>

<details>
<summary>Tier 3 — builder/cancel (zero swarm conflict) — <b>MERGED via PR #10</b></summary>

High value. Builder code path is untouched by our swarm drop — no swarm surgery
needed. `state.rs` is the one shared hotspot but hunks are in separate regions
from the swarm-drop deletions (clean append). As predicted: only 1 conflict
across all 7 commits (in `helpers/builder.rs`, builder usage selection logic).
Branch: `upstream/tier-3-builder-cancel` off the tier 2 branch.

| Upstream commit                            | Summary                                                                                                         | Notes                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| ------------------------------------------ | --------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `18a310064c9243a8b88f26226750418d59ba800f` | `CancelProcedure`/`CancelAction` Resolve impls + `CancelCache`/`CancellationToken` threading into execute layer | `state.rs` append is clean (different region from swarm drop). Adds `CancelProcedure`/`CancelAction` to `Execution` enum in `helpers/procedure.rs`.                                                                                                                                                                                                                                                                                                                                        |
| `13ae29eda4de0e36024f4d07dfa09b4f710e13fa` | Finalize update on procedure error                                                                              | Trivial fix paired with `18a310064`.                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `43ca25e55b5fdcf57ca47560bfd060eba49efacf` | Extend build cancel to server-type builders (was AWS-only)                                                      | Periphery `build_cancel_cache` + `CancelBuild` Resolve impl + `CancellationToken` through `CommandOptions::cancel(...)`.                                                                                                                                                                                                                                                                                                                                                                   |
| `ec65db12f37d5829be61d237f2e7d8d21dfd5d92` | Build webhook: cancel-then-rebuild instead of holding `build_locks` Mutex                                       | Better behavior for back-to-back pushes. Polls `CancelBuild` then re-triggers.                                                                                                                                                                                                                                                                                                                                                                                                             |
| `50e04187cf52cf644be4c69fd888787a36e8eaa7` | `BuilderConfig::Server` field `server_id: String` → `server_ids: Vec<String>`                                   | Multi-server build distribution. Touches `sync/toml.rs`, `resource/builder.rs`, `connection/server.rs`, `read/mod.rs`, `write/build.rs`.                                                                                                                                                                                                                                                                                                                                                   |
| `77cdd11c51c6e3f78ed9c91d2b8d94413d4b0e37` | TOML accepts `server`/`servers` aliases for `server_ids`                                                        | Companion to `50e04187c`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `ba7e0137735d5ac3ec570f25a1eadee748e060b9` | Replace naive `building % len` with refcount-based `BuilderUsage` selection                                     | Race-safe for concurrent builds: `HashMap<server_id, count>` + `min_by_key` + `release()` on cleanup. **Conflict resolution**: HEAD had manual build/repo state counting (from commits 1-2); upstream replaced with `builder_usage_cache()` refcount. Took upstream side — drops `Build`/`BuildState`/`Repo`/`RepoState`/`list_all_resources`/`build_state_cache`/`repo_state_cache` imports. `BuildCleanupData::Server` variant changed to `Server(Option<String>)` to carry usage token. |

### Tier 3 chronological order

Applied in this order (not the table order above): `50e04187c` → `77cdd11c5` →
`ba7e01377` → `18a310064` → `13ae29eda` → `43ca25e55` → `ec65db12f`.

</details>

---

## Deferred — future candidates

Revisit after further upstream or fork-side progress.

### Tier 4 — depends on Tier 2 infrastructure (now available)

Tier 2 pagination landed, so these are unblocked.

| Commit(s)                               | Summary                                                                | Notes                                                                      |
| --------------------------------------- | ---------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| `8f7854599` + `ac9037d84` + `6513cf297` | State filtering on list routes (cached state → in-memory repagination) | Med value. Drop swarm hunk in `ac9037d84`. Builds on Tier 2 `ListPermits`. |
| `a4b4b9bfc` + `47287fadb`               | Resource sorting (db-level + in-memory)                                | Med value. Drop swarm. Builds on Tier 2 + `97ce46d89`.                     |
| `3850b3596`                             | Non-semver image tag support in deploy flow                            | Med-High. Needs `entities/build.rs` methods ported together.               |
| `2d10b9e79`                             | Sync alerter ids→names fix (new `ReplaceIds` trait)                    | Med-High. New module + `Swarm` impl block to drop per Rule 1.              |

### Tier 5 — mechanical / low-risk / defer

| Commit(s)                                                                                                                               | Summary                                                      | Notes                                                                                                                  |
| --------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------- |
| `6ca10c9e5`                                                                                                                             | De-vendor from Docker (API renames, serde aliases)           | Med. 61 files, mostly mechanical. Couples to `mogh-ui` 0.7.0 types — port together if updating UI.                     |
| `e6448bdc7`                                                                                                                             | Cargo.toml `authors` removal + mogh dep bumps                | Low. Trivial/mechanical.                                                                                               |
| `1dc4602f7`                                                                                                                             | Rust 1.96.1 toolchain bump                                   | Low. Adopt when convenient.                                                                                            |
| `1d4a209e8`                                                                                                                             | `DeleteTerminal` name→id resolution                          | Low-Med. Our terminal target is usually `Server`-only.                                                                 |
| `5ab736154`                                                                                                                             | Action rename `deploy-fe`→`deploy-ui`, `dkf`→`dku`           | Low. Workflow-only.                                                                                                    |
| UI bundle (`ffa10ce02`, `43cc9ef9d`, `2b09fc17e`, `9b75b632c`, `7fd099e3b`, `ac23de398`, `9a14fb392`, `efa7e5c16`, `a53b2edbb`, et al.) | Paginated UI, omni-search, multi-selector, refetch-intervals | Low-Med. Port after Rust layer lands. Cherry-pick `9a14fb392` (multi-selector) + `efa7e5c16` (refetch-interval) first. |

### Explicitly skipped (swarm-only)

| Commit      | Reason                                             |
| ----------- | -------------------------------------------------- |
| `52a38f39f` | Swarm pagination + cli read support — Drop Rule 1. |
| `023754bbd` | Deployment/stack filter by swarm id — Drop Rule 1. |
| `05da009ce` | Update permissions on swarm — Drop Rule 1.         |

---

## Maintenance

When syncing from upstream again: update the Fork context table with the new
upstream tip, recompute the merge-base, and run
`git log --oneline <merge-base>..komodo/2.3.0` to enumerate new commits.
Cross-reference each new commit against the Drop rules before adding it to a
tier. Update the Merged / Deferred sections to reflect new state. </content>
</invoke>
