# Help Flags and Human Running Docs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use compose:subagent (recommended) or compose:execute to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the three control-plane binaries a proper `--help` (config surfaced as CLI flags with env-var defaults) and write `docs/running.md` so a human can build, run, and test the whole control plane.

**Architecture:** Go binaries switch from `os.Getenv` reads to stdlib `flag.String` whose default is taken from the existing env var; stdlib `flag` auto-handles `-h`/`--help` (exits 0). A small shared `internal/envutil` package holds the one-liner that picks env-or-fallback. The Rust sidecar hand-rolls a `parse_args` + `resolve_addr` (no new clap dep), prints `--help`, and exposes `--addr` with the same env-default semantics.

**Tech Stack:** Go 1.26.4 stdlib `flag`, Rust `std::env` + tokio/anyhow/axum (existing deps only), markdown docs.

**Design context (from approved brainstorm, no separate spec written — design lives in conversation):**
- Config: flags with env defaults (flag value defaults to env var if set, else built-in default).
- Go `flag` auto-generates `--help`; Rust hand-rolls (one flag, no clap).
- Startup banners: one `log.Printf`/`eprintln!` line printing resolved config so operators can tell what address/identity they got.
- Docs location: `docs/running.md` (full) + short README pointer.
- Out of scope: no new feature flags beyond existing config; no env var renames; no CI / Makefile.

**Build/test environment (set for every Go and cargo command):**
```bash
export TMPDIR=/home/acheong/.tmp/gotmp
export CARGO_TARGET_DIR=/home/acheong/.cargo-target
mkdir -p "$TMPDIR" "$CARGO_TARGET_DIR"
```

---

## File structure

- **Create:** `internal/envutil/envutil.go` — single function `EnvOrDefault(key, fallback string) string`; shared by both Go binaries.
- **Create:** `internal/envutil/envutil_test.go` — table tests for unset/empty/set env.
- **Modify:** `cmd/luddite-master/main.go` — replace `os.Getenv` with `flag.String(...envutil.EnvOrDefault(...))`; add startup banner.
- **Modify:** `cmd/luddite-agent/main.go` — same pattern; add required-field validation for `--root` and `--node-id`; add startup banner.
- **Modify:** `rust/iroh-bridge/src/main.rs` — add `parse_args`, `resolve_addr`, `HELP` constant; wire into `main()`; add `eprintln!` banner.
- **Create:** `docs/running.md` — prerequisites, build, full config table, topology startup, curl API examples, test commands.
- **Modify:** `README.md` — short pointer to `docs/running.md` under "Current Milestone".

---

## Task 1: Shared env helper (internal/envutil)

**Files:**
- Create: `internal/envutil/envutil.go`
- Test: `internal/envutil/envutil_test.go`

- [ ] **Step 1: Write the failing test**

Create `internal/envutil/envutil_test.go`:

```go
package envutil

import (
	"os"
	"testing"
)

func TestEnvOrDefault_unsetOrEmpty_returns_fallback(t *testing.T) {
	t.Setenv("LUDDITE_TEST_ENVUTIL", "")
	if got := EnvOrDefault("LUDDITE_TEST_ENVUTIL", "fallback"); got != "fallback" {
		t.Fatalf("empty: got %q, want %q", got, "fallback")
	}
	os.Unsetenv("LUDDITE_TEST_ENVUTIL")
	if got := EnvOrDefault("LUDDITE_TEST_ENVUTIL", "fallback"); got != "fallback" {
		t.Fatalf("unset: got %q, want %q", got, "fallback")
	}
}

func TestEnvOrDefault_set_returns_value(t *testing.T) {
	t.Setenv("LUDDITE_TEST_ENVUTIL", "from-env")
	if got := EnvOrDefault("LUDDITE_TEST_ENVUTIL", "fallback"); got != "from-env" {
		t.Fatalf("got %q, want %q", got, "from-env")
	}
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
TMPDIR=/home/acheong/.tmp/gotmp go test ./internal/envutil/...
```
Expected: FAIL — `undefined: EnvOrDefault` (package doesn't compile).

- [ ] **Step 3: Implement the helper**

Create `internal/envutil/envutil.go`:

```go
package envutil

import "os"

// EnvOrDefault returns the named environment variable's value, or fallback if
// the variable is unset or empty.
func EnvOrDefault(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}
```

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
TMPDIR=/home/acheong/.tmp/gotmp go test ./internal/envutil/...
```
Expected: PASS (`ok ... [no tests to run]` would mean the file path is wrong; expect `ok ...`).

- [ ] **Step 5: Commit**

```bash
git add internal/envutil/envutil.go internal/envutil/envutil_test.go
git commit -m "feat: add envutil.EnvOrDefault helper for flag defaults"
```

---

## Task 2: Master binary `--help` and startup banner

**Files:**
- Modify: `cmd/luddite-master/main.go` (full file rewrite)

- [ ] **Step 1: Rewrite main.go using flag + envutil.EnvOrDefault**

Replace the entire contents of `cmd/luddite-master/main.go` with:

```go
package main

import (
	"context"
	"flag"
	"log"
	"net/http"
	"time"

	masterhttp "github.com/luddite-dev/deploy/internal/master/httpapi"
	"github.com/luddite-dev/deploy/internal/master/state"
	"github.com/luddite-dev/deploy/internal/envutil"
	"github.com/luddite-dev/deploy/internal/sidecar/client"
)

func main() {
	statePath := flag.String("state",
		envutil.EnvOrDefault("LUDDITE_MASTER_STATE", "luddite-master.state.json"),
		"path to the master state file (env LUDDITE_MASTER_STATE)")
	sidecarAddr := flag.String("sidecar",
		envutil.EnvOrDefault("LUDDITE_MASTER_SIDECAR", "127.0.0.1:7777"),
		"address of the local iroh-bridge sidecar (env LUDDITE_MASTER_SIDECAR)")
	flag.Parse()

	store, err := state.Open(*statePath)
	if err != nil {
		log.Fatal(err)
	}
	sidecar := client.New(*sidecarAddr)
	masterEndpointAddr, err := sidecar.Identity(context.Background())
	if err != nil {
		log.Fatal(err)
	}

	log.Printf("luddite-master: state=%s sidecar=%s http=:8080 master-addr=%s",
		*statePath, *sidecarAddr, masterEndpointAddr)

	go func() {
		for {
			observed, err := sidecar.PollObserved(context.Background())
			if err == nil {
				for _, obs := range observed {
					_ = store.PutObservedDeployment(obs)
				}
			}
			time.Sleep(time.Second)
		}
	}()

	handler := masterhttp.New(store, sidecar, masterEndpointAddr)
	if err := http.ListenAndServe(":8080", handler); err != nil {
		log.Fatal(err)
	}
}
```

- [ ] **Step 2: Verify it builds**

Run:
```bash
TMPDIR=/home/acheong/.tmp/gotmp go build ./cmd/luddite-master
```
Expected: no output, exit 0. (Note: gofmt may sort imports; run `gofmt -w` if needed.)

- [ ] **Step 3: Verify `--help` works and prints flags**

Run:
```bash
./luddite-master --help 2>&1 | grep -E 'state|sidecar'
```
Expected: two lines containing `-state` and `-sidecar` with the env-var name in their usage text, e.g.:
```
  -sidecar string
    	address of the local iroh-bridge sidecar (env LUDDITE_MASTER_SIDECAR) (default "127.0.0.1:7777")
  -state string
    	path to the master state file (env LUDDITE_MASTER_STATE) (default "luddite-master.state.json")
```
Also confirm exit code is 0:
```bash
./luddite-master --help >/dev/null 2>&1; echo "exit=$?"
```
Expected: `exit=0`.

- [ ] **Step 4: Run full Go test suite to confirm no regressions**

Run:
```bash
TMPDIR=/home/acheong/.tmp/gotmp go test ./...
```
Expected: PASS for all packages (envutil, master/httpapi, master/state, agent/reconcile, sidecar/client).

- [ ] **Step 5: Commit**

```bash
git add cmd/luddite-master/main.go
git commit -m "feat: add --help and flags to luddite-master"
```

---

## Task 3: Agent binary `--help`, required-field validation, and startup banner

**Files:**
- Modify: `cmd/luddite-agent/main.go` (full file rewrite)

- [ ] **Step 1: Rewrite main.go using flags + envutil + required validation**

Replace the entire contents of `cmd/luddite-agent/main.go` with:

```go
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"net/http"
	"time"

	"github.com/luddite-dev/deploy/internal/agent/reconcile"
	"github.com/luddite-dev/deploy/internal/agent/runtime"
	"github.com/luddite-dev/deploy/internal/envutil"
	"github.com/luddite-dev/deploy/internal/sidecar/client"
)

type registerNodeRequest struct {
	NodeID       string `json:"node_id"`
	EndpointAddr string `json:"endpoint_addr"`
}

type registerNodeResponse struct {
	MasterEndpointAddr string `json:"master_endpoint_addr"`
}

func main() {
	sidecarAddr := flag.String("sidecar",
		envutil.EnvOrDefault("LUDDITE_AGENT_SIDECAR", "127.0.0.1:7777"),
		"address of the local iroh-bridge sidecar (env LUDDITE_AGENT_SIDECAR)")
	root := flag.String("root",
		envutil.EnvOrDefault("LUDDITE_AGENT_ROOT", ""),
		"path to the agent's deployment working root (env LUDDITE_AGENT_ROOT)")
	nodeID := flag.String("node-id",
		envutil.EnvOrDefault("LUDDITE_NODE_ID", ""),
		"unique id for this node, registered with the master (env LUDDITE_NODE_ID)")
	masterAPI := flag.String("master-api",
		envutil.EnvOrDefault("LUDDITE_MASTER_API", "http://127.0.0.1:8080"),
		"URL of the master HTTP API (env LUDDITE_MASTER_API)")
	flag.Parse()

	if *root == "" {
		log.Fatal("--root (env LUDDITE_AGENT_ROOT) is required")
	}
	if *nodeID == "" {
		log.Fatal("--node-id (env LUDDITE_NODE_ID) is required")
	}

	sidecar := client.New(*sidecarAddr)
	reconciler := reconcile.New(*root, runtime.Podman{})

	agentEndpointAddr, err := sidecar.Identity(context.Background())
	if err != nil {
		log.Fatal(err)
	}
	log.Printf("luddite-agent: node-id=%s root=%s sidecar=%s master-api=%s agent-addr=%s",
		*nodeID, *root, *sidecarAddr, *masterAPI, agentEndpointAddr)

	masterEndpointAddr, err := registerWithMaster(*masterAPI, *nodeID, agentEndpointAddr)
	if err != nil {
		log.Fatal(err)
	}

	for {
		desired, err := sidecar.PollDesired(context.Background())
		if err != nil {
			log.Print(err)
			time.Sleep(time.Second)
			continue
		}
		for _, dep := range desired {
			obs, err := reconciler.Apply(context.Background(), dep)
			if err != nil {
				log.Print(err)
				continue
			}
			if err := sidecar.ReportObserved(context.Background(), masterEndpointAddr, obs); err != nil {
				log.Print(err)
			}
		}
		time.Sleep(time.Second)
	}
}

func registerWithMaster(masterAPI, nodeID, endpointAddr string) (string, error) {
	body, err := json.Marshal(registerNodeRequest{NodeID: nodeID, EndpointAddr: endpointAddr})
	if err != nil {
		return "", err
	}
	res, err := http.Post(masterAPI+"/nodes/register", "application/json", bytes.NewReader(body))
	if err != nil {
		return "", err
	}
	defer res.Body.Close()
	if res.StatusCode >= 300 {
		return "", fmt.Errorf("register status %d", res.StatusCode)
	}
	var out registerNodeResponse
	if err := json.NewDecoder(res.Body).Decode(&out); err != nil {
		return "", err
	}
	return out.MasterEndpointAddr, nil
}
```

- [ ] **Step 2: Verify it builds**

Run:
```bash
TMPDIR=/home/acheong/.tmp/gotmp go build ./cmd/luddite-agent
```
Expected: no output, exit 0. Run `gofmt -w cmd/luddite-agent/main.go` to sort imports if needed.

- [ ] **Step 3: Verify `--help` works and prints all 4 flags**

Run:
```bash
./luddite-agent --help 2>&1 | grep -E 'sidecar|root|node-id|master-api'
```
Expected: four lines for `-sidecar`, `-root`, `-node-id`, `-master-api`, each naming the env var in its description. Exit code:
```bash
./luddite-agent --help >/dev/null 2>&1; echo "exit=$?"
```
Expected: `exit=0`.

- [ ] **Step 4: Verify required-field validation rejects missing root/node-id**

Run:
```bash
./luddite-agent 2>&1; echo "exit=$?"
```
Expected: stderr contains `--root (env LUDDITE_AGENT_ROOT) is required` and `exit=1`.

- [ ] **Step 5: Run full Go test suite**

Run:
```bash
TMPDIR=/home/acheong/.tmp/gotmp go test ./...
```
Expected: PASS for all packages.

- [ ] **Step 6: Commit**

```bash
git add cmd/luddite-agent/main.go
git commit -m "feat: add --help, flags, and required-field validation to luddite-agent"
```

---

## Task 4: Rust sidecar `--help` and `--addr` flag (TDD)

**Files:**
- Modify: `rust/iroh-bridge/src/main.rs` (full file rewrite)

- [ ] **Step 1: Replace main.rs with the test-inclusive version**

Replace the entire contents of `rust/iroh-bridge/src/main.rs` with:

```rust
use std::{net::SocketAddr, time::Duration};

use anyhow::{anyhow, Result};
use tokio::net::TcpListener;

use iroh_bridge::{http::router, network::Network, state::AppState};

const DEFAULT_ADDR: &str = "127.0.0.1:7777";

const HELP: &str = "\
luddite iroh-bridge: Iroh transport sidecar for the luddite control plane.

USAGE:
    iroh-bridge [--addr <ADDR>] [--help]

OPTIONS:
        --addr <ADDR>    Local address to bind the sidecar HTTP API on.
                         Defaults to the env variable LUDDITE_SIDECAR_ADDR
                         or 127.0.0.1:7777 if unset.
    -h, --help           Print this help and exit.
";

enum Parsed {
    Run { addr: Option<String> },
    Help,
}

fn parse_args(args: &[String]) -> Result<Parsed> {
    let mut addr: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--addr" => {
                i += 1;
                let v = args
                    .get(i)
                    .ok_or_else(|| anyhow!("--addr requires a value"))?;
                addr = Some(v.clone());
            }
            s if s.starts_with("--addr=") => {
                addr = Some(s["--addr=".len()..].to_string());
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
        i += 1;
    }
    Ok(Parsed::Run { addr })
}

fn resolve_addr(flag: Option<String>, env: Option<String>) -> String {
    flag.or(env)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_ADDR.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = std::env::var("LUDDITE_SIDECAR_ADDR").ok();
    match parse_args(&args)? {
        Parsed::Help => {
            print!("{HELP}");
            Ok(())
        }
        Parsed::Run { addr: flag } => {
            let raw_addr = resolve_addr(flag, env);
            let bind_addr: SocketAddr = raw_addr.parse()?;
            let state = AppState::new(String::new());
            let network = Network::bind(state.clone()).await?;
            network.refresh_identity().await?;
            eprintln!("iroh-bridge: sidecar http on {bind_addr}");

            tokio::spawn({
                let network = network.clone();
                async move {
                    loop {
                        if let Err(e) = network.flush_outbound_once().await {
                            eprintln!("flush_outbound: {e}");
                        }
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }
            });

            let listener = TcpListener::bind(bind_addr).await?;
            axum::serve(listener, router(state)).await?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolve_no_flag_no_env_uses_default() {
        assert_eq!(resolve_addr(None, None), DEFAULT_ADDR);
    }

    #[test]
    fn resolve_env_when_no_flag() {
        assert_eq!(
            resolve_addr(None, Some("127.0.0.1:9999".into())),
            "127.0.0.1:9999"
        );
    }

    #[test]
    fn resolve_flag_overrides_env() {
        assert_eq!(
            resolve_addr(
                Some("127.0.0.1:8888".into()),
                Some("127.0.0.1:9999".into())
            ),
            "127.0.0.1:8888"
        );
    }

    #[test]
    fn resolve_empty_env_uses_default() {
        assert_eq!(resolve_addr(None, Some(String::new())), DEFAULT_ADDR);
    }

    #[test]
    fn parse_help_short_and_long() {
        assert!(matches!(parse_args(&args(&["--help"])).unwrap(), Parsed::Help));
        assert!(matches!(parse_args(&args(&["-h"])).unwrap(), Parsed::Help));
    }

    #[test]
    fn parse_addr_space_separated() {
        match parse_args(&args(&["--addr", "127.0.0.1:7000"])).unwrap() {
            Parsed::Run { addr } => assert_eq!(addr.as_deref(), Some("127.0.0.1:7000")),
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn parse_addr_equals_form() {
        match parse_args(&args(&["--addr=127.0.0.1:7000"])).unwrap() {
            Parsed::Run { addr } => assert_eq!(addr.as_deref(), Some("127.0.0.1:7000")),
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn parse_addr_without_value_errors() {
        assert!(parse_args(&args(&["--addr"])).is_err());
    }

    #[test]
    fn parse_unknown_arg_errors() {
        assert!(parse_args(&args(&["--bogus"])).is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they pass (with cargo env vars)**

Run:
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp \
  cargo test --manifest-path rust/iroh-bridge/Cargo.toml --bin iroh-bridge
```
Expected: 8 passed in the `tests` module inside `main.rs` (resolve_* x4 + parse_* x4). Note: the loopback + http_smoke integration tests are in `tests/` and may also run if not filtered; both should still pass.

- [ ] **Step 3: Verify the binary builds and `--help` works**

Run:
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp \
  cargo build --manifest-path rust/iroh-bridge/Cargo.toml
./rust/iroh-bridge/../../.cargo-target/debug/iroh-bridge --help
```
Expected: prints the `HELP` text starting with `luddite iroh-bridge:` and listing `--addr` and `--help`. Exit code 0. (If the binary's path varies, locate it via `find /home/acheong/.cargo-target -name iroh-bridge -type f -executable`.)

- [ ] **Step 4: Verify `--addr` flag binds to the specified address**

Run (in one terminal):
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target \
  ./.cargo-target/debug/iroh-bridge --addr 127.0.0.1:7011 &
sleep 1
curl -s http://127.0.0.1:7011/health || true
kill %1 2>/dev/null || true
```
Expected: the binary logs `iroh-bridge: sidecar http on 127.0.0.1:7011` and curl gets an HTTP response (the `/health` route behavior is whatever the existing router returns; non-connection-refused counts as success).

- [ ] **Step 5: Run full Rust test suite to confirm no regressions**

Run:
```bash
CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp \
  cargo test --manifest-path rust/iroh-bridge/Cargo.toml
```
Expected: all tests pass (4 http_smoke + 1 iroh_loopback + 8 new unit tests in main.rs).

- [ ] **Step 6: Commit**

```bash
git add rust/iroh-bridge/src/main.rs
git commit -m "feat: add --help and --addr flag to iroh-bridge sidecar"
```

---

## Task 5: Human docs — docs/running.md and README pointer

**Files:**
- Create: `docs/running.md`
- Modify: `README.md` (append one bullet under "Current Milestone")

- [ ] **Step 1: Create docs/running.md**

Create `docs/running.md` with the content below. (Routes and JSON shapes are taken verbatim from `internal/master/httpapi/server.go` and `internal/control/types.go` — do not invent fields.)

````markdown
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

| Flag        | Env var                  | Default                       | Notes                                   |
|-------------|--------------------------|-------------------------------|-----------------------------------------|
| `--state`   | `LUDDITE_MASTER_STATE`   | `luddite-master.state.json`  | Path to the master's persistent state.  |
| `--sidecar` | `LUDDITE_MASTER_SIDECAR` | `127.0.0.1:7777`              | Address of the local iroh-bridge.       |

The HTTP API always listens on `:8080`.

### `luddite-agent`

| Flag            | Env var                  | Default                  | Notes                                              |
|-----------------|--------------------------|--------------------------|----------------------------------------------------|
| `--sidecar`     | `LUDDITE_AGENT_SIDECAR`  | `127.0.0.1:7777`         | Address of the local iroh-bridge.                  |
| `--root`        | `LUDDITE_AGENT_ROOT`     | _(required)_             | Working root for the agent's deployments.          |
| `--node-id`     | `LUDDITE_NODE_ID`        | _(required)_             | Unique node id registered with the master.         |
| `--master-api`  | `LUDDITE_MASTER_API`     | `http://127.0.0.1:8080`  | Base URL of the master HTTP API.                   |

`--root` and `--node-id` are required; the agent exits 1 if either is empty.

### `iroh-bridge`

| Flag     | Env var                | Default           | Notes                                              |
|----------|------------------------|-------------------|----------------------------------------------------|
| `--addr` | `LUDDITE_SIDECAR_ADDR` | `127.0.0.1:7777`  | Local address to bind the sidecar's HTTP API on.   |

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
````

- [ ] **Step 2: Add the README pointer**

In `README.md`, under the `## Current Milestone` section, after the existing bullet list (before "Persistent storage, backups..."), insert a single bullet:

```markdown
- See [`docs/running.md`](docs/running.md) to build, run, and test the control plane end-to-end.
```

The result is the existing bullet list plus one new bullet pointing at the running guide.

- [ ] **Step 3: Sanity-check the docs render and links resolve**

Run:
```bash
ls docs/running.md && grep -c 'docs/running.md' README.md
```
Expected: docs/running.md exists and README.md contains the reference (count >= 1).

- [ ] **Step 4: Commit**

```bash
git add docs/running.md README.md
git commit -m "docs: add human running and testing guide for the control plane"
```

---

## Final verification

After Tasks 1-5:

- [ ] **Full Go build + test:**
  ```bash
  TMPDIR=/home/acheong/.tmp/gotmp go build ./... && TMPDIR=/home/acheong/.tmp/gotmp go test ./...
  ```
  Expected: build succeeds, all packages pass.

- [ ] **Full Rust build + test:**
  ```bash
  CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp cargo build --manifest-path rust/iroh-bridge/Cargo.toml && \
  CARGO_TARGET_DIR=/home/acheong/.cargo-target TMPDIR=/home/acheong/.tmp/gotmp cargo test --manifest-path rust/iroh-bridge/Cargo.toml
  ```
  Expected: build succeeds, all tests pass.

- [ ] **All three binaries respond to `--help`:**
  ```bash
  ./luddite-master  --help >/dev/null 2>&1; echo "master=$?"
  ./luddite-agent   --help >/dev/null 2>&1; echo "agent=$?"
  ./.cargo-target/debug/iroh-bridge --help >/dev/null 2>&1; echo "bridge=$?"
  ```
  Expected: `master=0`, `agent=0`, `bridge=0`.

## Self-review

- **Spec coverage:** No formal spec doc was written for this small milestone (design lives in the approved conversation). All design items (config via flags-with-env-defaults for 3 binaries, Rust hand-rolled `--help`, startup banners, `docs/running.md` + README pointer) are covered by Tasks 1-5.
- **Placeholder scan:** Every step contains concrete code or commands; no TBD/TODO/edge-case handwaving.
- **Type consistency:** `EnvOrDefault(key, fallback string) string` (Go) and `resolve_addr(flag: Option<String>, env: Option<String>) -> String` (Rust) signatures are stable across tasks that reference them. Flag names (`--state`, `--sidecar`, `--root`, `--node-id`, `--master-api`, `--addr`) match across the binary code in Tasks 2-4 and the config tables in Task 5.
```

The plan is written. Self-review is at the bottom. Now I'll move to execution handoff — per the plan skill, check memory for a saved `execution-style` preference.<tool_call>bash<arg_key>command</arg_key><arg_value>mkdir -p /home/acheong/.tmp/gotmp /home/acheong/.cargo-target 2>&1 && echo "dirs ready"