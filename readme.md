# Luddite Deploy

> [!NOTE]
>
> This project is a hard fork of [Komodo](https://github.com/moghtech/komodo)
> (GPL-V3). The original Komodo README is preserved in the details block below.

I currently have most of my infrastructure self-hosted on Kubernetes. It was a
lot of effort to set up, there are lots of footguns, and the way storage is
dealt with has caused more downtime than it has saved.

The goal of this project is to distill down my Kubernetes workflow & simplify it
for small self-hosted deployments with built-in support for persistent storage,
backups, DNS management, HTTPS, and port allocation. We also aim to use podman
instead of docker for rootless and out of general preference.

The overall vibe should be "podman compose, but with everything else around it
taken care of".

This is a fork of [Komodo](https://komo.do/), chosen as it provides a solid
open-source foundation for what I need: Core/Periphery architecture, GitOps
sync, and working with containers. With Komodo, we:

- Drop swarm mode. We handle orchestration rather than relying on docker.
- Take control of the placement scheduler. We auto-assign deployments to nodes
  by default, so that we can seamlessly move to another node when one is
  decommissioned.
- S3-backed volume store allows exporting/importing named volumes so that we can
  automate backups and restore data onto other nodes when necessary
- Iroh p2p transport replaces the original websocket + Noise handshake protocol.
  Both because Iroh is cool and I often have nodes without public IP addresses.

> ![NOTE]
>
> AI-use disclosure: A significant portion of the code has been written by AI,
> specifically `GLM-5.2` with
> [Mimocode](https://github.com/XiaomiMiMo/MiMo-Code). I then proceeded to give
> the agent access to VPS machines for actual deployment and testing. I
> personally find the code quality to be abhorrent, but it does somehow work.
> Initially intended as an experiment, but I am slowly adopting it myself. The
> S3 backups are completely separate from the system & directly compatible with
> podman imports, and so hopefully if anything does go wrong, I can restore
> things with minimal damage. This is not intended for production.

## Quickstart

### Deploying the Core

First, copy `./compose.yml` onto a server. Since we use
[Iroh](https://www.iroh.computer/) for networking, this server doesn't need to
have a public IP. Then, create a `.env` file in the same directory and copy over
the contents from `./.env.example`. Make sure to change
`KOMODO_INIT_ADMIN_PASSWORD` as by default, the UI is publicly exposed.

For now, automatic DNS & HTTPS only works with Cloudflare. To configure it, do
`mkdir -p config/keys` and paste your Cloudflare API token (with permissions to
write to DNS) into `config/keys/cloudflare-token` and enter your top level
domain for which subdomains will be created upon. We currently do not support
multiple domains yet.

### Get the Onboarding Key and Public Key

First, we need an onboarding key from the core to ensure the periphery is
authorized. Go to `http://<core-ip>:9210` and log in with the credentials in
`.env` (Default is `admin:changeme`)

On the sidebar, click on "Settings". Then click on the dropdown (Says
"Variables" initially) and select "Onboarding". Then click "New Onboarding Key".
Copy that key.

The public key should be shown at the top of the same page above the dropdown
menu.

### Deploying the Periphery

The Periphery is where containers are actually deployed. I haven't figured out
podman in podman yet, so for now you should deploy on the host itself. In theory
you should be able to mount the podman socket and have a compose for this too
but you'd still be limited to 1 periphery instance per host.

- Grab a [release binary](https://github.com/luddite-dev/deploy/releases) and
  drop it on your server.

If running as non-root user:

- `sudo loginctl enable-linger $USER` to keep sockets and services alive after
  logout
- Ensure podman sockets are enabled:
  `systemctl --user enable --now podman.socket`

If running as root:

- `systemctl enable --now podman.socket`

Then

- In the same directory, create `periphery.env`

```bash
PERIPHERY_CORE_ENDPOINT_ADDRS="xxx" # Core Public Key
PERIPHERY_ONBOARDING_KEY="xxx" # Onboarding Key
PERIPHERY_CONNECT_AS="Primary Node" # Arbitrary name
PERIPHERY_ROOT_DIRECTORY="$HOME/.local/share/luddite"
PERIPHERY_INGRESS_ENABLED=true # Or false. I think you need root for ingress to get port 80 and 443, but there are ways to allow a non-root access to reserve these ports as well.
PERIPHERY_HTTP_BRIDGE_PORT=8443 # Change to a different port if occupied
```

The periphery should auto-detect which podman socket to use depending on whether
we're root or not, but you can also set
`DOCKER_HOST=unix:///var/run/docker.sock` manually.

You can finally run the periphery binary. Set it up as a `systemd` service or
just run it in `screen`.

### TODO: Document basic example of compose stack

<details>

<summary>

Milestones

</summary>

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

## Milestone 4 — Caddy + DNS ingress (landed)

Automatic HTTPS for user-deployed web apps — no manual reverse proxy, no manual
TLS certs, no manual DNS. Deploy a container with an HTTP proxy config and get a
routable `https://<subdomain>.<base_domain>` URL automatically:

- **Caddy reverse proxy** — vendored static binary (xcaddy-built with the
  Cloudflare DNS plugin) running as a host process on designated ingress nodes.
  Configured entirely via JSON through Caddy's admin API (`POST /load`), no
  Caddyfile. Hot reload on every deployment change — zero downtime.
- **Cloudflare DNS management** — trait-abstracted `DnsProvider` interface with
  Cloudflare as the first implementation. Core creates/updates/deletes A records
  for app subdomains automatically. Idempotent — re-deploys detect existing
  records instead of failing.
- **Iroh HTTP bridge (data plane)** — in-process axum HTTP listener on the
  ingress Periphery opens Iroh QUIC streams to worker Periphery nodes. When the
  ingress node is also the worker (single-node setup), traffic short-circuits
  directly to `127.0.0.1:<port>` without going through Iroh.
- **TLS via ACME DNS-01** — Caddy obtains certificates using Cloudflare DNS-01
  challenges (no port 80 needed for ACME). The same Cloudflare API token is used
  for both record management and certificate issuance.
- **Ingress failover** — when an ingress node goes down, Core migrates DNS
  records to a healthy ingress node. Server state is tracked in an in-memory
  cache (refreshed every 15s via `PollStatus` health checks) and persisted to
  the DB on state transitions.
- **Vendored binary pipeline** — separate
  [`luddite-dev/vendored`](https://github.com/luddite-dev/vendored) repo with
  daily CI that checks for new Caddy releases, builds with xcaddy, and publishes
  release assets with SHA256 checksums + a manifest.json version manifest.
  Periphery auto-detects and downloads binary updates.

End-to-end verified: deploy → DNS A record created → Caddy config pushed → HTTPS
reverse proxy working → subdomain modify → deployment delete → DNS records
cleaned up.

Design spec:
[`docs/compose/specs/2026-07-12-caddy-dns-ingress-design.md`](docs/compose/specs/2026-07-12-caddy-dns-ingress-design.md)
· Implementation plan:
[`docs/compose/plans/2026-07-12-caddy-dns-ingress.md`](docs/compose/plans/2026-07-12-caddy-dns-ingress.md)

</details>

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
