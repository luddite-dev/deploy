# Running and testing the control plane

This guide covers building, configuring, running, and testing the first-milestone control plane: the Go `luddite-master`, the Go `luddite-agent`, and the Rust `iroh-bridge` sidecar.

## Prerequisites

- **Go** 1.26 or newer
- **Rust** toolchain (stable) — `rustup` is fine
- **Podman** 4+ (only needed on agent nodes; the reconcile loop shells out to `podman compose`)
- **curl** (for the API examples below)

Set these for every Go/cargo invocation so the small tmpfs is not exhausted:

```bash
export TMPDIR=/home/acheong/.tmp/gotmp
export CARGO_TARGET_DIR=/home/acheong/.cargo-target
mkdir -p "$TMPDIR" "$CARGO_TARGET_DIR"
```

## Building

```bash
# Go binaries land in the current directory.
go build -o luddite-master ./cmd/luddite-master
go build -o luddite-agent  ./cmd/luddite-agent

# Rust binary lands in $CARGO_TARGET_DIR/debug/iroh-bridge.
cargo build --manifest-path rust/iroh-bridge/Cargo.toml
```

## Configuration

Every config value is a CLI flag whose default comes from an env var if set, else a built-in default. Run `<binary> --help` to see the full list for that binary.

### `luddite-master`

| Flag        | Env var                  | Default                       | Notes                                    |
|-------------|--------------------------|-------------------------------|------------------------------------------|
| `--state`   | `LUDDITE_MASTER_STATE`   | `luddite-master.state.json`  | Path to the master's persistent state.   |
| `--sidecar` | `LUDDITE_MASTER_SIDECAR` | `127.0.0.1:7777`              | Address of the local iroh-bridge.        |

The HTTP API always listens on `:8080`.

### `luddite-agent`

| Flag           | Env var                  | Default                  | Notes                                              |
|----------------|--------------------------|--------------------------|----------------------------------------------------|
| `--sidecar`    | `LUDDITE_AGENT_SIDECAR`  | `127.0.0.1:7777`         | Address of the local iroh-bridge.                  |
| `--root`       | `LUDDITE_AGENT_ROOT`     | _(required)_             | Working root for the agent's deployments.          |
| `--node-id`    | `LUDDITE_NODE_ID`        | _(required)_             | Unique node id registered with the master.         |
| `--master-api` | `LUDDITE_MASTER_API`     | `http://127.0.0.1:8080`  | Base URL of the master HTTP API.                   |

`--root` and `--node-id` are required; the agent exits 1 if either is empty.

### `iroh-bridge`

| Flag     | Env var                | Default           | Notes                                              |
|----------|------------------------|-------------------|----------------------------------------------------|
| `--addr` | `LUDDITE_SIDECAR_ADDR` | `127.0.0.1:7777`  | Local address to bind the sidecar's HTTP API on.  |

`--help` / `-h` prints usage and exits 0.

## Running the topology

The control plane is four processes started in this order. Run each in its own terminal.

**1. Master sidecar** (master machine):

```bash
./iroh-bridge --addr 127.0.0.1:7777
```

**2. Master** (master machine, after the sidecar comes up):

```bash
./luddite-master --state ./luddite-master.state.json --sidecar 127.0.0.1:7777
```

Startup logs `luddite-master: state=... sidecar=... http=:8080 master-addr=<iroh endpoint>`. The HTTP API is now on `:8080`.

**3. Agent sidecar** (agent node):

```bash
./iroh-bridge --addr 127.0.0.1:7777
```

**4. Agent** (agent node, registers with the master on startup):

```bash
./luddite-agent \
  --sidecar 127.0.0.1:7777 \
  --root ./agent-root \
  --node-id worker-1 \
  --master-api http://127.0.0.1:8080
```

The agent prints its resolved config, fetches its Iroh endpoint address from its sidecar, then `POST /nodes/register`s itself with the master. From there it polls desired deployments every second, runs them via `podman compose`, and reports observed state back over the Iroh link.

### One-machine smoke run

On a single host for testing, run steps 1-4 in four terminals. The agent and master need not share a filesystem; they communicate over Iroh and HTTP.

## Exercising the API

All examples assume the master is on `http://127.0.0.1:8080` and the agent registered as `worker-1`.

**Register a node** (what the agent does on startup):

```bash
curl -sS -X POST http://127.0.0.1:8080/nodes/register \
  -H 'content-type: application/json' \
  -d '{"node_id":"worker-1","endpoint_addr":"<agent-iroh-endpoint>"}'
# -> {"master_endpoint_addr":"<master-iroh-endpoint>"}
```

**List registered nodes:**

```bash
curl -sS http://127.0.0.1:8080/nodes
# -> [{"node_id":"worker-1","endpoint_addr":"...","connected":true}]
```

**Create a deployment** (the body is a Compose file as a string):

```bash
curl -sS -X POST http://127.0.0.1:8080/nodes/worker-1/deployments/web \
  -H 'content-type: application/json' \
  -d '{"compose_yaml":"services:\n  web:\n    image: nginx:alpine\n    ports:\n      - 8081:80\n"}'
# -> {"node_id":"worker-1","version":1,"spec":{"name":"web","compose_yaml":"..."},"deleted":false}
```

**Check deployment status** (combined desired + observed view):

```bash
curl -sS http://127.0.0.1:8080/nodes/worker-1/deployments/web
# -> {"desired":{...},"observed":{"node_id":"worker-1","name":"web","applied_version":1,"state":"succeeded"}}
```

States are `"pending"`, `"succeeded"`, or `"failed"`. After the agent reconciles, `state` moves from `pending` to `succeeded` or `failed`.

**Delete a deployment** (writes a tombstone; the agent tears it down):

```bash
curl -sS -X DELETE http://127.0.0.1:8080/nodes/worker-1/deployments/web
# -> {"node_id":"worker-1","version":2,"spec":{...},"deleted":true}
```

## Testing

### Go tests

```bash
TMPDIR=/home/acheong/.tmp/gotmp go test ./...
```

Covers the master state store, master HTTP API, agent reconcile loop, and sidecar client.

### Rust tests

```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp \
  cargo test --manifest-path rust/iroh-bridge/Cargo.toml
```

Includes the local HTTP smoke tests, the `parse_args` / `resolve_addr` unit tests in `src/main.rs`, and the Iroh loopback transport test. The loopback test takes roughly 3 seconds because the Iroh endpoints discover relay URLs via `endpoint.online().await` before the address resolves; that wait is expected.
