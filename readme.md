# Luddite Deploy

> [!NOTE] This project is a hard fork of
> [Komodo](https://github.com/moghtech/komodo) (GPL-V3). The original Komodo
> README is preserved in the details block below.

Kubernetes is just a tad too complicated, podman itself a bit too limited.

The goal of this project is to simplify small self-hosted deployments with
built-in support for persistent storage, backups, DNS, HTTPS, and port-based
allocation. It is a wrapper around Podman, striving to use existing standards
where possible rather than creating things from scratch (e.g. compose for
multi-service deployments).

## Why fork Komodo?

Komodo provides the closest existing open-source foundation for what luddite
needs: a Core/Periphery desired-state control plane, GitOps sync, mTLS auth, and
a REST API with OpenAPI. Rather than rebuild from scratch, we fork and adapt:

- **Drop Swarm mode** — small self-hosted deployments don't need a second
  orchestration model. This removes `swarm_id`, the `docker stack` executor, and
  Swarm-only entity types.
- **Typed port and volume config** — replace Komodo's free-text `String` fields
  with structured `Vec<PortMapping>` and `Vec<VolumeMount>`. Bind mounts are
  unrepresentable by typing; only named volumes are allowed.
- **Placement scheduler** — auto-assign deployments to Periphery nodes based on
  host-port availability, instead of requiring operators to manually pin each
  deployment to a server. `server_id` becomes an optional hint.
- **S3-backed volume lifecycle** — export/import named volumes to S3-compatible
  storage via `podman volume export`/`import`. Enables node draining with data
  migration: backup volumes on the source, restore on the target, then deploy.
- **Node draining** — mark a server `Drain`; Core walks its deployments and
  migrates them to other nodes with volume data intact.
- **Iroh p2p transport** — replace Komodo's WebSocket + mutual Noise handshake
  transport with [Iroh](https://iroh.computer) (QUIC + TLS 1.3 with raw public
  keys). Core and Periphery are Iroh endpoints identified by Ed25519
  `EndpointId`s. Periphery always dials Core (unified direction). Onboarding is
  a bearer token over the first Iroh stream, validated against DB records.
  Eliminates self-signed SSL cert generation, the Noise XX handshake, and the
  bidirectional Core→Periphery / Periphery→Core duality (Iroh's NAT traversal
  handles all topologies including Core behind NAT).

## Milestone 1 — adaptive placement (landed)

The first milestone added three interlocking capabilities, all merged to `main`:

1. **Placement scheduler & port allocation** — Core picks a target node by
   probing free host ports via `netstat2` (`/proc/net/tcp` reads, no shell-out).
   HTTP-proxied services get a random high port; fixed-port services (DNS, SSH)
   land only on nodes where those ports are free.
2. **Volume management & backup/restore** — named-volumes-only enforcement,
   S3-backed export/import, on-demand and scheduled backups.
3. **Node draining & migration orchestration** — operator marks a server
   `Drain`; Core orchestrates backup → restore → deploy → stop migrations to
   other nodes.

Design spec:
[`docs/compose/specs/2026-06-25-adaptive-placement-design.md`](docs/compose/specs/2026-06-25-adaptive-placement-design.md)
· Implementation plan:
[`docs/compose/plans/2026-06-25-adaptive-placement.md`](docs/compose/plans/2026-06-25-adaptive-placement.md)

Out of scope for this milestone: Iroh transport swap, Caddy
reverse-proxy integration (only the data contract for assigned ports).

## Milestone 3 — Iroh transport swap (landed)

Replaced the WebSocket + mutual Noise XX handshake transport layer entirely with
an Iroh-native transport. No adapter, no double-auth:

- **Transport:** Iroh QUIC with built-in TLS 1.3 (RFC 7250 Raw Public Keys).
  Mutual authentication is automatic on every connection; MITM prevention is
  inherent. No X.509, no CA, no self-signed SSL cert generation.
- **Identity:** Each Core/Periphery has an Iroh `EndpointId` (Ed25519 public
  key, derived from a persisted `SecretKey`). Replaces `address`, `public_key`,
  and `passkey` fields on `ServerConfig`/`ServerInfo`.
- **Direction:** Unified Periphery→Core. Periphery always initiates. Eliminates
  the `ServerConfig.address` direction flag and the bidirectional connection
  model. Iroh NAT traversal (UDP hole-punching + relay fallback) handles all
  topologies.
- **Onboarding:** Bearer token over authenticated Iroh stream. Periphery sends
  the token as the first stream message; Core validates against
  `onboarding_keys` DB records and registers the Periphery's `EndpointId` on the
  `Server` entity.
- **What survives unchanged:** `TransportMessage` wire protocol (Request/
  Response/Terminal with UUID multiplexing), `PeripheryRequest` dispatch via
  `#[derive(Resolve)]`, `PeripheryConnection` channel routing.

What was deleted: `lib/transport/src/auth.rs` (Noise XX),
`lib/transport/src/ websocket/` (WS trait family),
`lib/transport/src/timeout.rs`, SSL cert generation in
`bin/periphery/src/helpers.rs`, Core's outbound dialer (`connection/client.rs`),
Periphery's inbound server (`connection/server.rs`).

Design spec:
[`docs/compose/specs/2026-07-12-iroh-transport-design.md`](docs/compose/specs/2026-07-12-iroh-transport-design.md)
· Implementation plan:
[`docs/compose/plans/2026-07-12-iroh-transport.md`](docs/compose/plans/2026-07-12-iroh-transport.md)

<details>
<summary>Original Komodo README</summary>

# Komodo 🦎

A tool to build and deploy software across many servers.

🦎 [See the docs](https://komo.do)

🦎 [Try the Demo](https://demo.komo.do) - Login: `demo` : `demo`

🦎 [See the Build Server](https://build.mogh.tech) - Login: `komodo` : `komodo`

🦎 [Join the Discord](https://discord.gg/DRqE8Fvg5c)

## About

The Komodo dragon is the largest living member of the
[_Monitor_ family of lizards](https://en.wikipedia.org/wiki/Monitor_lizard).

There is no limit to the number of servers you can connect, and there will never
be. There is no limit to what API you can use for automation, and there never
will be.

## Disclaimer

Warning. This is open source software (GPL-V3), and while we make a best effort
to ensure releases are stable and bug-free, there are no warranties. Use at your
own risk.

## Links

- [periphery setup](https://github.com/moghtech/komodo/blob/main/scripts/readme.md)
- [roadmap](https://github.com/moghtech/komodo/blob/main/roadmap.md)

## Screenshots

### Light Theme

![Dashboard](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Light-Dashboard.png)
![Stack](https://raw.githubusercontent.com/moghtech/komodo/blob/main/screenshots/Light-Stack.png)
![Compose](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Light-Compose.png)
![Env](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Light-Env.png)
![Sync](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Light-Sync.png)
![Update](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Light-Update.png)
![Stats](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Light-Stats.png)
![Export](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Light-Export.png)

### Dark Theme

![Dashboard](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Dark-Dashboard.png)
![Stack](https://raw.githubusercontent.com/moghtech/komodo/blob/main/screenshots/Dark-Stack.png)
![Compose](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Dark-Compose.png)
![Env](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Dark-Env.png)
![Sync](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Dark-Sync.png)
![Update](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Dark-Update.png)
![Stats](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Dark-Stats.png)
![Export](https://raw.githubusercontent.com/moghtech/komodo/main/screenshots/Dark-Export.png)

</details>
