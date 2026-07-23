# Stack HTTP Proxy / Ingress / Domain Allocation

Date: 2026-07-23
Status: Approved (brainstorm) → spec

## [S1] Problem

HTTP proxy (automatic DNS + Caddy reverse proxy + HTTPS) is implemented for
`Deployment` resources but not for `Stack` (compose) resources. A stack is the
natural unit for the project's "embedded Caddy" pattern — a multi-service
compose project with one reverse-proxy service that should be the only
externally exposed service. Today there is no way to give a stack an
automatic subdomain + HTTPS endpoint.

The gap has three layers:

1. `StackConfig` has no `http_proxy` field (and stacks need an extra
   `service` selector that deployments don't, since a stack has many
   services).
2. Per-service host-port readback is explicitly deferred in
   `bin/core/src/resource/stack.rs:286` — container names are only known
   after `ComposeUp` returns them, so the deployment-style `post_create`
   readback can't work as-is.
3. `build_ingress_routes` (Caddy config builder) and the DNS record
   lifecycle (`create_deployment_dns_record` /
   `delete_deployment_dns_records`) are wired exclusively to the
   `deployments` collection.

There is also no UI for `http_proxy` on **either** deployments or stacks.
The deployment-side UI is out of scope for this change; only the stack
config UI is added.

## [S2] Solution overview

Add a single optional `http_proxy: Option<StackHttpProxyConfig>` to
`StackConfig`, where the config selects one service + subdomain +
container port. Wire it into the stack lifecycle:

- After `DeployStack::resolve` finishes the `ComposeUp` call (which
  returns service container names), read back host ports for the one
  proxied service, then set up DNS + Caddy.
- On stack delete, tear down DNS + rebuilt Caddy.
- On stack update with a changed `http_proxy`, tear down old records and
  set up new ones (best-effort, like deployments).
- Extend `build_ingress_routes` to also emit routes for stacks.
- Add an "HTTP Proxy" config group to the stack config UI.

This mirrors the deployment ingress flow (`try_setup_ingress` /
`try_teardown_ingress` / `build_ingress_routes`) at
`bin/core/src/resource/deployment.rs`, adapted for the stack's
multi-service / late-container-name-discovery reality.

## [S3] Data model

```rust
// client/core/rs/src/entities/stack.rs
#[typeshare]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct StackHttpProxyConfig {
  /// Which compose service to proxy to. Must match a service name
  /// declared in the compose file. Resolved to a container name via
  /// `StackServiceNames.container_name` returned by `ComposeUp`.
  pub service: String,
  /// Subdomain. FQDN = "{subdomain}.{ingress.dns.base_domain}".
  pub subdomain: String,
  /// Which container port on that service receives proxied traffic.
  pub container_port: u16,
}
```

On `StackConfig`:

```rust
/// HTTP ingress configuration for this stack.
/// When set, creates a DNS record + Caddy route for automatic HTTPS,
/// pointing at one service's container port.
#[serde(default, skip_serializing_if = "Option::is_none")]
#[partial_attr(serde(default))]
#[builder(default)]
pub http_proxy: Option<StackHttpProxyConfig>,
```

This follows the exact attribute pattern used by `DeploymentConfig.http_proxy`
(`bin/core/src/entities/deployment.rs:266-271`) so `partial_derive` /
`diff_derive` / `builder` all behave consistently.

### DnsRecord generalization

`DnsRecord` (`client/core/rs/src/entities/dns.rs:33`) currently has
`deployment_id: Option<String>`. Add a parallel field:

```rust
/// The stack this record is attached to, if any.
#[serde(skip_serializing_if = "Option::is_none")]
pub stack_id: Option<String>,
```

Additive and non-breaking: existing deployment records keep
`deployment_id`, new stack records use `stack_id`. Teardown queries
filter on the appropriate field. This avoids overloading
`deployment_id` with stack ids (which would be semantically wrong and
break the deployment teardown query).

## [S4] Host-port readback (the deferred prerequisite)

`bin/core/src/resource/stack.rs:286` (the `post_create` TODO) defers
per-service host-port readback because container names aren't known
until after deploy. We resolve this by doing the readback inside
`DeployStack::resolve`, *after* `ComposeUp` returns the service list.

In `bin/core/src/api/execute/stack.rs`, after the existing
`update_info` block (~line 283) and before `refresh_server_cache`:

1. If `stack.config.http_proxy` is `Some(proxy)`:
2. Find the matching `StackServiceNames` in the returned `services` Vec
   where `service_name == proxy.service`.
3. Call `ReadContainerPorts { container_name }` on the stack's server
   periphery (same API deployments use at
   `bin/core/src/resource/deployment.rs:467`).
4. Persist the result into `info.host_ports[proxy.service]` via a
   `$set` update on the `stacks` collection.

Only the proxied service is read back — not all services — so this is
one periphery round-trip per deploy with `http_proxy` set.

If the service name doesn't match any returned service (misconfiguration),
log a warning and skip ingress setup; the deploy itself still succeeds.

### Where the container name comes from

`StackServiceNames.container_name` (entities/stack.rs:786) stores the
compose-project-prefixed name without the replica suffix, e.g.
`mystack-app`. `ReadContainerPorts` on the periphery matches containers
by name; the periphery-side matching uses a regex
`^container_name-?[0-9]*$` so replicas are covered. This matches the
existing pattern — no new periphery logic needed.

## [S5] Lifecycle

### Deploy setup

New function in `bin/core/src/resource/stack.rs` (mirroring
`try_setup_ingress` in deployment.rs:544):

```rust
pub async fn try_setup_stack_ingress(
  stack: &Stack,
  http_proxy: &StackHttpProxyConfig,
) -> anyhow::Result<()>
```

Steps (identical structure to deployment's `try_setup_ingress`):

1. Read `core_config().ingress`, bail if no `base_domain`.
2. Build FQDN = `{subdomain}.{base_domain}`.
3. Resolve the stack's assigned server (`stack.config.server_id` or
   `stack.info.assigned_server`) → `endpoint_id`. Bail if empty.
4. Look up the proxied service's host port from
   `stack.info.host_ports[http_proxy.service]`, finding the entry where
   `p.container == http_proxy.container_port`. Bail with a helpful
   message if not found (readback may have failed).
5. `select_new_ingress_node("")` → ingress node + its cached public IPs.
6. `create_stack_dns_record(stack_id, subdomain, ingress_node.id, ipv4,
   ipv6, &core_cfg, 60)` — new function in
   `bin/core/src/ingress/management.rs`, parallel to
   `create_deployment_dns_record` but persisting `stack_id` instead of
   `deployment_id`.
7. `build_ingress_routes(base_domain, &token)` → `build_caddy_config`
   → `ReloadCaddyConfig` to the ingress periphery.

This is called from `DeployStack::resolve` after the host-port readback
(S4). Best-effort: failures are logged at `warn!` and do not fail the
deploy (same as deployments, deployment.rs:243-248).

### Update teardown+setup

`post_update` (`bin/core/src/resource/stack.rs:346`) currently delegates
to `post_create`. Add explicit http_proxy change handling:

- Compare the pre-update and post-update `http_proxy`. If anything
  changed (added, removed, or any field differs):
  - `try_teardown_stack_ingress(stack_id)` to delete old DNS records +
    push Caddy without the old route.
  - If the new `http_proxy` is `Some`, `try_setup_stack_ingress` after
    the host-port readback has run (which only happens on deploy, not
    on bare config update — see note below).

**Note on update timing:** A bare `UpdateStack` (config change without
a redeploy) does not run `ComposeUp`, so containers aren't recreated and
host ports may be stale or absent. For the common case (user adds
`http_proxy` and then deploys), setup happens at deploy time via S4. If
the user changes `http_proxy` on an already-deployed stack without
redeploying, teardown of the old route runs immediately, but setup of
the new route will only complete on the next `DeployStack`. This is
acceptable and matches the "ingress follows deploy" model. A
log message tells the operator to redeploy.

### Delete teardown

`post_delete` (`bin/core/src/resource/stack.rs:437`): add a best-effort
call to `try_teardown_stack_ingress(&resource.id)` when
`resource.config.http_proxy.is_some()`, mirroring deployment's
`post_delete` (deployment.rs:370-377).

New function in `bin/core/src/resource/stack.rs`:

```rust
async fn try_teardown_stack_ingress(stack_id: &str) -> anyhow::Result<()>
```

Calls `delete_stack_dns_records(stack_id, &core_cfg)` (new, parallel to
`delete_deployment_dns_records`) which queries `dns_records` by
`stack_id`, deletes each at the provider, then deletes the db rows.
Then rebuilds + pushes Caddy without the removed route.

## [S6] Caddy route integration

`build_ingress_routes` (`bin/core/src/resource/deployment.rs:720`)
currently queries only the `deployments` collection
(`doc! { "config.http_proxy": { "$ne": null } }`). Extend it to also
query stacks:

```rust
let stacks: Vec<Stack> = find_collect(
  &db_client().stacks,
  doc! { "config.http_proxy": { "$ne": null } },
  None,
).await?;

for stack in stacks {
  let Some(http_proxy) = &stack.config.http_proxy else { continue; };
  // resolve server → endpoint_id (skip if missing)
  // look up stack.info.host_ports[http_proxy.service],
  //   find p.container == http_proxy.container_port
  // push CaddyRoute { hostname, target_endpoint_id, target_port }
}
```

Deployment and stack routes are collected into the same `Vec<CaddyRoute>`
and pushed as one Caddy config. The existing "rebuild all routes on any
change" model is preserved — every create/update/delete across both
resource types pushes a complete config. No incremental diffing.

## [S7] UI

Add an "HTTP Proxy" config group to
`ui/src/resources/stack/config/index.tsx`, in the main (non-advanced)
section. Shown only in Server mode (`!currSwarmId`), matching the
deployment-side expectation.

Fields:

- **Service**: a Mantine `Combobox` (or `AutoComplete`) populated from
  `stack.info.deployed_services ?? stack.info.latest_services` service
  names. Before the first deploy this list is empty, so the field
  accepts free-text input — the user types the service name they intend
  to declare in the compose file. After deploy, the known services
  appear as suggestions.
- **Subdomain**: `TextInput`. A helper line below shows the resulting
  FQDN: `→ https://{subdomain}.{base_domain}`. The `base_domain` comes
  from `GetCoreInfo` (the core's `ingress.dns.base_domain`). If ingress
  is not configured on the core (no base domain), render the group
  disabled with a hint: "Configure ingress DNS on the Core first."
- **Container Port**: `NumberInput`, min 1.

When `http_proxy` is set, show a read-only status line at the top of the
group: `Configured endpoint: https://{subdomain}.{base_domain}`. This
is derived from config + core info — no new backend read endpoint.

The group is added to the `""` (main) groups array in each mode branch
(UI Defined / Files On Server / Git Repo) before `generalCommon`.

### Deployment UI (out of scope)

The deployment resource also has no UI for its existing `http_proxy`
field. Adding it is out of scope for this change to keep the work
focused on stacks. Flagged as a conscious exclusion.

## [S8] Validation

In `validate_config` (`bin/core/src/resource/stack.rs:447`), add (only
when `config.http_proxy` is `Some` in the partial):

- `service` non-empty.
- `subdomain` non-empty, lowercase, DNS-label-safe (letters, digits,
  hyphens; no leading/trailing hyphen; ≤63 chars). Reuse a small
  validator helper.
- `container_port > 0`.
- **Subdomain uniqueness**: query both `deployments` (where
  `config.http_proxy.subdomain == value`) and `stacks` (same) for any
  resource that already claims the same subdomain. On **create**, any
  match is a conflict. On **update**, exclude the stack being updated by
  its id (`_id != self.id`) so a no-op re-save doesn't trip the check.
  Reject with a clear error naming the conflicting resource if a
  conflict exists. This prevents two resources fighting over one DNS
  record / Caddy hostname. Deployment-side validation is not changed in
  this task (scope discipline); the shared check still catches a stack
  conflicting with an existing deployment.

## [S9] Out of scope

- Deployment-side `http_proxy` UI (see S7).
- Adding `http_proxy` uniqueness validation to the deployment
  `validate_config` (only stack-side validation is added).
- Swarm-mode stack ingress (server mode only, matching the rest of the
  ingress layer which assumes Iroh endpoint routing).
- Multiple proxy entries per stack (single entry covers the embedded
  Caddy pattern; per-service list deferred unless a real need emerges).
- DNS record migration / renaming of `deployment_id` (additive
  `stack_id` field only).
