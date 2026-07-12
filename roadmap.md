# Roadmap

This is a hard fork of [Komodo](https://github.com/moghtech/komodo) (GPL-V3).
The milestones below track the fork's own direction; Komodo's upstream
release history is preserved for reference where still relevant.

If you have an idea, open an issue beginning with the `[Request]` tag. PRs
fulfilling any planned milestone are welcome.

## Fork milestones

- **M1 — Adaptive placement** ✅ — placement scheduler with host-port probing,
  typed port/volume config (named volumes only), S3-backed volume
  backup/restore, node draining with migration orchestration.
- **M2 — Upstream sync (Tiers 1–3)** ✅ — ZFS ARC memory stats, pagination
  backbone (`ListPermits`), builder cancel + multi-server distribution. See
  [`docs/forking.md`](docs/forking.md).
- **M3 — Iroh transport swap** ✅ — replaced WebSocket/mutual Noise transport
  with Iroh p2p (QUIC + TLS 1.3, raw public keys, unified Periphery→Core
  direction, bearer token onboarding). See
  [`docs/compose/specs/2026-07-12-iroh-transport-design.md`](docs/compose/specs/2026-07-12-iroh-transport-design.md).
- **M4 — MongoDB replacement** — swap Mongo for an embedded store suited to
  small self-hosted deployments. Out of scope until M3 lands.
- **M5 — Caddy reverse-proxy integration** — consume the assigned-port data
  contract to auto-configure Caddy for HTTP-proxied services.

## Upstream release history (Komodo, inherited)

- **v1.12**: Support any git provider / docker registry (supports self-hosted providers like Gitea) ✅
- **v1.13**: Support "Compose" resource - Paste in a docker compose file and manage it like a Portainer "Stack" ✅
- **v1.14**: Manage docker networks, images, volumes in the UI ✅
- **v1.15**: Support generic OIDC providers (including self-hosted) ✅
- **v1.16**: "Action" resource: Run requests on the Komodo API using snippets of typescript. ✅
- **v1.17**: Procedure Schedules: Run procedures / Actions at scheduled times, like CRON job. Connect to host terminals and exec into containers ✅
- **v1.18**: Upgrade granular role based access control system ✅
- **v2.0**: Support "Swarm" resource — ⛔ **dropped in this fork** (see
  `docs/forking.md`, Drop Rule 1). Replaced by the `server_id`-only model and
  the M1 placement scheduler.
- **Undecided**: Support "Cluster" resource - Manage Kubernetes cluster, can attach deployments to "Cluster" — not planned for this fork.

**Note.** The fork does NOT follow Komodo's version numbers.