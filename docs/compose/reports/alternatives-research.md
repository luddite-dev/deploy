# Open-source, Kubernetes-independent self-hosted deployment alternatives

> Research report for `luddite-deploy`.
>
> Scope: existing open-source systems that deploy services to 1–10 of your own servers without requiring Kubernetes, and how well they match luddite deploy's goal of a Podman-first, multi-node control plane with persistent storage, backups, DNS/HTTPS and port allocation.

## TL;DR recommendation

**If luddite deploy's goal is a reusable Podman-first control plane, the strongest existing match is [Komodo](https://github.com/moghtech/komodo).** It is fully open source (GPL-V3), Rust/TypeScript, built around a central Core + Periphery-agent architecture, supports multi-server deployments, GitOps, a REST API, and allows Docker/Podman via a `podman → docker` alias. It already does much of what the luddite milestone is trying to wire up (desired-state resource sync across nodes, build pipelines, audit logging, CLI/web UI). Its main gaps vs luddite's stated ambitions are: no built-in reverse proxy, DNS or automatic HTTPS (it expects Caddy/Traefik/Nginx/Cloudflare in front), no built-in storage backup system, and Podman support is a compatibility shim rather than first-class.

**If you are willing to use Docker and want the most mature off-the-shelf PaaS, [Coolify](https://github.com/coollabsio/coolify)** is the best maintained (57k+ GitHub stars, Apache 2.0, multi-server, built-in databases, Let's Encrypt, S3 backups, 280+ one-click services). It is conceptually close to luddite but Docker-only and much larger in scope.

**Nothing in this survey is a drop-in replacement for a Podman-native, compose-centric, multi-node control plane** that also handles its own HTTPS, DNS, storage lifecycle and backups out of the box. That combination is genuinely underserved, which is the best argument for continuing luddite deploy as its own project.

## Evaluation criteria

| Criterion | Why it matters for luddite deploy |
|-----------|-----------------------------------|
| **Open-source license (OSI-compatible)** | User requirement. |
| **Container runtime** | luddite wants Podman. Docker-only tools require a hard dependency tradeoff. |
| **Multi-node / clustering** | Current milestone uses master↔agent model. |
| **Deployment model** | Git-push, UI, API, CLI, declarative GitOps — different workflows. |
| **HTTPS / DNS automation** | Stated luddite goal. |
| **Persistent storage & backups** | Stated luddite goal. |
| **Active maintenance / maturity** | ecosystem, security updates, community size. |
| **Scope / complexity** | Smaller tools fit small teams better; large PaaS bring concepts luddite wants to avoid. |

## Comparison matrix

| Project | License | Runtime | Multi-node | Deploy model | HTTPS/DNS | Storage/backup | Maturity | Fit |
|---------|---------|---------|------------|--------------|-----------|----------------|----------|-----|
| **Coolify** | Apache 2.0 | Docker | Yes (multi-server, Swarm) | Git, UI, API, compose | Let's Encrypt, manual A | S3 DB backups, volumes | Very high | Good if Docker OK |
| **CapRover** | Apache 2.0 | Docker/Swarm | Yes (Swarm) | UI, CLI, git, `captain-definition` | Let's Encrypt | Volumes; BYO backup | High | Docker-only PaaS |
| **Dokku** | MIT | Docker | Single-node | `git push`, CLI, buildpacks | Let's Encrypt plugin | Plugins; BYO | High | Mini-Heroku, not multi-node |
| **Dokploy** | Apache 2.0 (+ proprietary dir) | Docker/Swarm | Yes (Swarm) | Git, UI, API, compose | Traefik/Let's Encrypt | Auto DB backups | Medium-high | Like Coolify, Docker-only |
| **Piku** | MIT | Python/uwsgi | Single-node | `git push` | Virtual hosts; BYO certs | Host FS; BYO | Low | Tiny, not container-based |
| **Komodo** | GPL-V3 | Docker / Podman alias | Yes (Core + agents) | GitOps TOML, UI, API, CLI | BYO proxy | Compose volumes; BYO | Medium | **Closest to luddite architecture** |
| **Kamal** | MIT | Docker (Podman plugin experimental) | Multi-host SSH | YAML + CLI | Let's Encrypt via proxy | Bind mounts; BYO | High | Stateless web apps |
| **Portainer CE** | zlib | Docker/Swarm/K8s/**Podman** | Yes (environments/Edge) | UI, API, compose, GitOps polling | BYO | Volumes; BYO | Very high | Best Podman container manager, not PaaS |
| **CasaOS** | Apache 2.0 | Docker | Single-node | App store | BYO | Volumes; BYO | Medium | Home NAS, not pipeline |
| **RunTipi** | GPL-V3 | Docker | Single-node | App store | BYO | Volumes; BYO | Medium | Homeserver app store |
| **YunoHost** | AGPL-V3 | Debian packages | Single-node | App packages | Built-in Let's Encrypt | OS-level backups | High | Server OS, not containers |
| **Umbrel** | PolyForm Noncommercial (non-OSI) | Docker | Single-node | App store | BYO | Volumes; BYO | Medium | Source-available home server |
| **Homarr** | Apache 2.0 | Docker | n/a | Dashboard | n/a | n/a | Medium | Dashboard only |
| **Nomad** | BSL 1.1 (not OSI) | Podman driver / Docker / exec / VM | Yes | HCL jobspecs, API | BYO | CSI plugins | Very high | Disqualified by license |
| **Easypanel** | unclear/closed source | Docker | Limited; paid Swarm | UI, git | Let's Encrypt (?) | Volumes; BYO | Unknown | Not truly open source |

## Detailed profiles

### Coolify
- **What it is:** Self-hostable PaaS alternative to Vercel/Heroku/Netlify. Deploy static sites, databases, full-stack apps and 280+ one-click services on your own servers.
- **GitHub:** `coollabsio/coolify` — Apache 2.0.
- **Runtime:** Docker only; users have patched it to run on Podman but it is not a supported target ([source](https://github.com/coollabsio/coolify/discussions/3137)).
- **Multi-node:** Yes. Manager server + remote servers via SSH; Docker Swarm supported ([source](https://coolify.io/docs)).
- **Deployment:** Git push/webhook, UI, API, custom Docker Compose files.
- **HTTPS/DNS:** Automatic Let's Encrypt; point an A record at the server.
- **Storage/backup:** Managed databases with S3-compatible backups; named volumes for app data.
- **Verdict:** Best maintained open-source PaaS in this space. If luddite can accept Docker and a large UI surface, Coolify would save years. Doesn't help the Podman preference.

### CapRover
- **What it is:** "Heroku on steroids" using Docker, nginx, Let's Encrypt, NetData.
- **GitHub:** `caprover/caprover` — Apache 2.0.
- **Runtime:** Docker; built on Docker Swarm. Podman support explicitly unlikely ([source](https://github.com/caprover/caprover/discussions/1451)).
- **Multi-node:** Yes via Swarm worker nodes.
- **Deployment:** Web UI, CLI, git webhooks, one-click apps, custom Dockerfiles.
- **HTTPS/DNS:** Built-in Let's Encrypt.
- **Storage/backup:** Persistent dirs via UI; backups are user-managed (Restic, scripts).
- **Verdict:** Mature and simpler than Coolify, but Docker/Swarm only.

### Dokku
- **What it is:** The "smallest PaaS" — Docker-powered mini-Heroku.
- **GitHub:** `dokku/dokku` — MIT.
- **Runtime:** Docker. Podman is discussed but unsupported ([source](https://github.com/dokku/dokku/issues/5515)).
- **Multi-node:** Single-node by design.
- **Deployment:** `git push dokku main`, Heroku buildpacks or Dockerfile, CLI.
- **HTTPS/DNS:** Let's Encrypt plugin.
- **Storage/backup:** Data services via plugins; plugin-dependent backups.
- **Verdict:** Excellent mini-Heroku for one box; not multi-node or Podman-first.

### Dokploy
- **What it is:** Newer self-hosted PaaS; Vercel/Netlify/Heroku alternative built with Docker and Traefik.
- **GitHub:** `Dokploy/dokploy` — Apache 2.0 but includes a `/proprietary` directory ([source](https://github.com/Dokploy/dokploy/blob/canary/LICENSE.MD)).
- **Runtime:** Docker / Docker Swarm.
- **Multi-node:** Yes, via Docker Swarm.
- **Deployment:** Git, UI, API, compose.
- **HTTPS/DNS:** Traefik/Let's Encrypt.
- **Storage/backup:** Automated DB backups to external storage.
- **Verdict:** Similar to Coolify, newer, partly proprietary, Docker-only.

### Piku
- **What it is:** "The tiniest PaaS"; git-push deployments for tiny servers.
- **GitHub:** `piku/piku` — MIT.
- **Runtime:** Python/nginx/uwsgi, not containers.
- **Multi-node:** No.
- **Deployment:** `git push piku master`.
- **HTTPS/DNS:** Virtual hosts only; BYO SSL.
- **Storage/backup:** Host filesystem; BYO.
- **Verdict:** Charming minimalism, but not container-based and not multi-node.

### Komodo — closest structural match
- **What it is:** "A tool to build and deploy software on many servers." GPL-V3. Rust core + periphery agents + TypeScript UI.
- **GitHub:** `moghtech/komodo` ([source](https://github.com/moghtech/komodo)).
- **Runtime:** Docker by default; Podman via documented `podman → docker` alias ([source](https://komo.do/docs/intro)).
- **Multi-node:** Native. One Core + any number of Periphery agents. Core and agents authenticate with passkeys/mTLS.
- **Deployment:** Resources as TOML files in Git and synced to Komodo (GitOps), plus UI, REST/WebSocket API, CLI, TypeScript/Rust clients ([source](https://komo.do/)).
- **HTTPS/DNS:** Not built in. Docs recommend Caddy/Traefik/Nginx/Cloudflare Tunnel in front ([source](https://github.com/moghtech/komodo/discussions/1319)).
- **Storage/backup:** Relies on Docker/Podman volumes and Compose. "Procedures" for arbitrary automation, but no built-in backup.
- **Verdict:** The closest open-source analogue to the luddite master/agent desired-state architecture. Its gaps are exactly the Podman-first runtime, built-in DNS/HTTPS, and storage/backup abstractions.

### Kamal
- **What it is:** Basecamp's deployment tool for containerized web apps over SSH.
- **GitHub:** `basecamp/kamal` — MIT.
- **Runtime:** Docker-first; community Podman plugin is experimental ([source](https://github.com/phoozle/kamal_podman/)).
- **Multi-node:** Yes, any number of servers via SSH; uses `kamal-proxy`.
- **Deployment:** Declarative `config/deploy.yml` + `kamal deploy` CLI.
- **HTTPS/DNS:** Kamal 2 has automatic Let's Encrypt via kamal-proxy for single-host DNS A-record ([source](https://kamal-deploy.org/docs/configuration/proxy/)).
- **Storage/backup:** Bind mounts; no built-in backup.
- **Verdict:** Best for stateless web apps; weak for multi-service compose and persistent storage.

### Portainer CE
- **What it is:** Lightweight container management UI/platform.
- **GitHub:** `portainer/portainer` — zlib license.
- **Runtime:** Docker, Swarm, Kubernetes, ACI, and Podman environments (with Podman-specific install/agent).
- **Multi-node:** Yes; add environments and optionally Edge agents.
- **Deployment:** UI, API, compose stacks, GitOps polling for stack updates.
- **HTTPS/DNS:** No built-in reverse proxy or DNS.
- **Storage/backup:** Volume management; BYO backup.
- **Verdict:** Excellent cross-runtime container control panel; not a PaaS, but the only widely-adopted tool that treats Podman as a first-class environment.

### Homelab / app-store systems
Not direct competitors to a multi-node control plane, but adjacent:

- **CasaOS:** Apache 2.0, Docker-based personal cloud/app store for single machines.
- **RunTipi:** GPL-V3, Docker-based homeserver app store.
- **YunoHost:** AGPL-V3, Debian-based server OS with app packages and built-in mail/Let's Encrypt.
- **Umbrel:** PolyForm Noncommercial 1.0.0 (source-available, **not OSI open source**). Home-server OS with app store.
- **Homarr:** Apache 2.0 dashboard/launcher, not a deployment tool.

## What existing systems do better than luddite deploy today

1. **Maturity and ecosystem.** Coolify, CapRover, Dokku and Portainer have years of real-world usage, UIs, one-click apps and extensive docs.
2. **Built-in HTTPS/Let's Encrypt.** Coolify, CapRover, Dokku, Dokploy and Kamal remove hand-rolled ACME.
3. **Database lifecycle.** Coolify and Dokploy spin up managed databases with backups.
4. **Multi-node orchestration.** Komodo and Portainer already have central panels that push state to remote agents.

## Where existing systems fall short of luddite deploy's goals

1. **Podman as a first-class runtime.** Every PaaS is Docker-first; at best they tolerate Podman via aliases. None treat rootless Podman + systemd + compose as the native model.
2. **Integrated DNS, port allocation and HTTPS.** Most hand DNS/reverse proxy off to Caddy/Traefik or expect manual A records.
3. **Storage and backup orchestration.** Existing systems manage volumes but not ZFS/btrfs lifecycle, snapshots, off-site restores.
4. **Small, composable control plane.** Coolify/Dokploy/Dokku are full-stack PaaS monoliths. Komodo is the only one that feels like a "control plane."

## Recommendation

**The research does not reveal an existing open-source system that already does exactly what luddite deploy wants to do.** If the team wants a ready-made PaaS and can accept Docker, **Coolify** is the pragmatic choice. If they want the closest open-source control-plane analogue, **Komodo** is the one to study, contribute to, or benchmark against.

### Strategic options

1. **Build luddite deploy anyway**, positioning it as the Podman-native, storage/DNS/backup-aware alternative to Komodo/Portainer. The gap is real and defensible.
2. **Contribute Podman-first runtime support and storage/backup primitives to Komodo.** It is GPL-V3, so luddite would need to be comfortable with copyleft.
3. **Fork/adopt Komodo** and specialize it for Podman + storage/DNS/HTTPS. Same license considerations.
4. **Build a plugin/tool on top of Portainer CE or Komodo** that adds the missing storage/backup/DNS abstractions. Lowest risk, less ownership of the core.

### Suggested decision

Unless you are happy to switch to Docker, **Option 1 is justified**, but treat **Komodo** as the primary competitor and **Portainer CE** as the Podman-compatible reference implementation. The current Iroh-backed master/agent design is a meaningful network differentiator in constrained environments, but Komodo's simpler HTTPS/agent model should be the benchmark for whether that complexity is worth it.

## Appendix: Komodo details — volumes, ports, networking

Komodo has two deployment resource types:

1. **Deployment** — a single container. The agent translates the config into a `docker run` command.
2. **Stack** — a Docker Compose project.

### Persistent storage (Deployment resource)

The `Deployment` resource has a `volumes` field that accepts **bind mounts** in `host_path:container_path` format. There is no dedicated managed-volume abstraction; persistence is host paths mounted into the container. If you want named volumes or more complex storage, you use a `Stack` instead.

Example from the docs:

```toml
[[deployment]]
name = "my-app"
[deployment.config]
server = "server-prod"
network = "host"
volumes = """
/data/my-app/data:/app/data
/data/my-app/config:/app/config
"""
```

See <https://komo.do/docs/deploy/containers>.

### Exposing ports (Deployment resource)

Port mappings are explicit `host_port:container_port` entries in the `ports` field. The default network is `host`, which bypasses Docker's port-mapping layer entirely. If you use a non-host network, you list mappings.

```toml
[deployment.config]
ports = ["27018:27017"]
```

There is no automatic port allocation, no built-in reverse proxy, and no automatic DNS/HTTPS. You decide which host port is exposed and reverse-proxy it yourself (Caddy, Traefik, nginx, Cloudflare Tunnel).

### Persistent storage & ports (Stack resource)

For multi-service apps, the `Stack` resource is a standard Docker Compose project. You provide `compose.yaml` either in the UI, from files on the host, or from a Git repo. Anything Compose supports — named volumes, bind mounts, port mappings, networks, secrets, configs — works. Komodo redeploys the project and can auto-redeploy on Git push via a webhook.

Example Stack config:

```toml
[[stack]]
name = "my-stack"
[stack.config]
server = "server-prod"
run_directory = "/opt/stacks/my-stack"
file_paths = ["compose.yaml"]
repo = "myorg/stacks"
environment = """
DB_HOST = db.example.com
LOG_LEVEL = info
"""
```

See <https://komo.do/docs/deploy/compose>.

### Networking across nodes

Komodo has no own overlay network. It relies on:

- **Host networking or Docker networks** on the local server for single-node stacks.
- **Docker Swarm** for cross-node service mesh (Stacks and Deployments can target a Swarm instead of a single server).
- **WireGuard / Tailscale / Cloudflare Tunnel** for private connectivity between Core, Periphery agents and operator access. The docs and community guides frequently combine Komodo with a WireGuard mesh.

So Komodo gives you a central control plane and lifecycle operations, but it does **not** provide: automatic DNS records, built-in reverse proxy, TLS termination, automatic port allocation, distributed storage, or backup orchestration. Those are all external concerns you bring yourself.

## Sources

- Coolify GitHub: <https://github.com/coollabsio/coolify>
- Coolify Podman discussion: <https://github.com/coollabsio/coolify/discussions/3137>
- CapRover GitHub: <https://github.com/caprover/caprover>
- CapRover Podman discussion: <https://github.com/caprover/caprover/discussions/1451>
- Dokku GitHub: <https://github.com/dokku/dokku>
- Dokku Podman issue: <https://github.com/dokku/dokku/issues/5515>
- Dokploy GitHub / license: <https://github.com/Dokploy/dokploy>, <https://github.com/Dokploy/dokploy/blob/canary/LICENSE.MD>
- Piku GitHub: <https://github.com/piku/piku>
- Komodo GitHub: <https://github.com/moghtech/komodo>
- Komodo docs / Podman note: <https://komo.do/docs/intro>
- Komodo reverse-proxy guidance: <https://github.com/moghtech/komodo/discussions/1319>
- Kamal GitHub: <https://github.com/basecamp/kamal>
- Kamal proxy / SSL docs: <https://kamal-deploy.org/docs/configuration/proxy/>
- Kamal Podman plugin: <https://github.com/phoozle/kamal_podman/>
- Portainer GitHub: <https://github.com/portainer/portainer>
- CasaOS GitHub: <https://github.com/IceWhaleTech/CasaOS>
- RunTipi GitHub: <https://github.com/runtipi/runtipi>
- YunoHost GitHub: <https://github.com/YunoHost/yunohost>
- Umbrel license FAQ: <https://github.com/getumbrel/umbrel/wiki/License-FAQ>
- Homarr GitHub: <https://github.com/homarr-labs/homarr>
- HashiCorp BSL adoption (Nomad license): <https://www.hashicorp.com/en/blog/hashicorp-adopts-business-source-license>
