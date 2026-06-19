# Multi-Node Control Plane Design

## [S1] Problem

`luddite deploy` aims to simplify self-hosted deployments on top of Podman. The first detailed milestone should focus on the multi-node control plane rather than full application-platform concerns.

The initial control-plane problem is: a master service must manage application deployments on remote slave nodes that may sit behind NAT or on public residential networks. The first milestone should support node-scoped application deployment only. Persistent storage, backups, DNS, HTTPS, and broader node administration remain out of scope.

## [S2] Solution Overview

The first milestone uses a declarative control model.

- The master stores desired deployment state per node.
- Slave agents keep outbound authenticated connections to the master using Iroh.
- Operators submit desired deployment changes through an HTTP API exposed by the master.
- Agents reconcile local Podman deployments toward the latest desired state and report observed status back.

This gives the project a control loop that tolerates intermittent node connectivity and fits remote machines behind NAT.

## [S3] Scope

In scope for this milestone:

- Registering and tracking remote nodes
- Maintaining an outbound agent-to-master control channel over Iroh
- Accepting application deployment intent through a master HTTP API
- Persisting versioned desired state per node
- Reconciling Podman/compose application deployments on slave nodes
- Reporting node liveness and deployment status back to the master
- Exposing desired and observed deployment status through the master API

Out of scope for this milestone:

- Persistent storage provisioning
- Backup orchestration
- DNS management
- HTTPS or certificate automation
- General-purpose remote shell or machine administration
- Automatic rollback after failed deployment attempts

## [S4] Architecture

The system has three primary parts.

Master service:

- Exposes the operator-facing HTTP API
- Persists node records, desired deployments, desired-state versions, and latest observed status
- Publishes node-specific desired state to connected agents
- Tracks heartbeats and last-seen information

Slave agent:

- Authenticates and maintains an outbound Iroh connection to the master
- Subscribes to desired state intended for its node
- Materializes deployment input locally
- Applies create, update, or remove operations through Podman and compose
- Reports the applied desired version, success or failure, and observed deployment status

Podman runtime on the node:

- Runs the application services defined by the current desired deployment state
- Remains the local execution layer; the agent is responsible for translating desired state into runtime actions

## [S5] Operator Surface

The first operator entrypoint is an HTTP API on the master.

This keeps the first milestone focused on control-plane semantics instead of CLI ergonomics or Git-driven workflows. A future CLI can become a thin client over the API without changing the underlying control model.

The API should support at least:

- Creating or updating a deployment spec for a target node
- Removing a deployment from a target node
- Listing known nodes and their liveness status
- Reading desired state, observed state, and last apply result for a node deployment

## [S6] State Model

The control plane is declarative and versioned.

- The master is the source of truth for desired intent.
- Each node deployment update produces a new desired-state version.
- Agents reconcile toward the newest version they can observe.
- Agents must be able to re-apply a desired version safely after reconnects or partial failures.
- The node agent is the source of truth for current observed runtime status on that node.

Desired state and observed state are intentionally separate. This allows the master to show both what should be running and what the node most recently reported as actually running.

## [S7] Data Flow

1. An operator submits a deployment spec to the master HTTP API for a target node.
2. The master validates the request, assigns a new desired-state version, persists it, and marks the deployment pending.
3. The connected slave agent receives or fetches the new node-specific desired state over Iroh.
4. The agent reconciles local Podman state to match the desired deployment version.
5. The agent reports success or failure, plus current observed status, back to the master.
6. The master exposes both desired and observed state through the API.

If a node is offline when desired state changes, the intended state remains queued on the master until the node reconnects.

## [S8] Failure Handling

The first milestone should keep failure behavior explicit and minimal.

- Offline nodes do not block new desired state from being recorded.
- Re-applying the same desired-state version must be idempotent.
- Failed applies remain visible as failed; the system does not attempt automatic rollback in v1.
- Recovery comes from retrying the same desired state or submitting a newer desired state.
- Connection loss does not discard intent because the master persists desired state independently of agent connectivity.

This keeps the first version understandable while still supporting eventual convergence after reconnects.

## [S9] Testing Strategy

The initial implementation plan should cover tests for:

- Master-side desired-state versioning and persistence behavior
- Validation and API behavior for deployment submission and removal
- Agent reconcile behavior when applying the same desired version repeatedly
- Agent handling of offline and reconnect scenarios
- Status reporting transitions for pending, success, and failure cases
- Integration coverage for master-agent synchronization over the chosen Iroh-based control path

## [S10] Open Follow-On Work

This milestone deliberately sets up later phases without solving them now.

Likely next phases after this control-plane slice:

- Storage and volume lifecycle integration
- Backup orchestration
- DNS and HTTPS automation
- CLI on top of the HTTP API
- Richer deployment packaging, history, and rollback semantics
