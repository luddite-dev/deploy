# Multi-Node Control Plane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use compose:subagent
> (recommended) or compose:execute to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first milestone control plane: a Go master accepts node
registrations and deployment intent over HTTP, a Go agent reconciles Podman
deployments, and a Rust sidecar on each side carries desired-state and
observed-status messages over Iroh.

**Architecture:** Keep application logic in Go and isolate Iroh inside a small
Rust sidecar process. The master owns versioned desired state, node records, and
the operator API; the agent owns reconcile logic and Podman execution; each Rust
sidecar owns Iroh connectivity plus a narrow local HTTP boundary for queueing
outbound messages and polling inbound ones.

**Tech Stack:** Go, standard library HTTP/testing, Podman Compose CLI, Rust
(stable >= 1.91 required for `iroh 1.0.0`), Tokio, Axum, Serde, Iroh.

---

## File Structure

- `go.mod`: Go module definition for the control-plane services.
- `internal/control/types.go`: Shared Go types for nodes, desired state,
  observed state, and API views.
- `internal/master/state/store.go`: File-backed persistence for node records,
  desired deployments, and observed deployments.
- `internal/master/state/store_test.go`: Persistence and versioning coverage for
  the store.
- `internal/master/httpapi/server.go`: Operator-facing HTTP API for node
  registration, node listing, deployment create/remove, and deployment status
  reads.
- `internal/master/httpapi/server_test.go`: API tests using `httptest` and a
  fake sidecar publisher.
- `internal/agent/runtime/podman.go`: Small wrapper around Podman Compose CLI
  operations.
- `internal/agent/reconcile/reconcile.go`: Idempotent reconcile logic for apply
  and delete operations.
- `internal/agent/reconcile/reconcile_test.go`: Reconcile behavior tests,
  including repeated versions and delete handling.
- `internal/sidecar/client/client.go`: Go client for the local Rust sidecar HTTP
  API.
- `internal/sidecar/client/client_test.go`: Client request/response tests.
- `cmd/luddite-master/main.go`: Wires the master store, API server, and poll
  loop for observed reports coming back from the sidecar.
- `cmd/luddite-agent/main.go`: Wires node registration, desired-state polling,
  reconcile, and observed-status reporting.
- `rust/iroh-bridge/Cargo.toml`: Rust crate manifest for the sidecar.
- `rust/iroh-bridge/src/lib.rs`: Library entrypoint for sidecar modules so Rust
  integration tests can import them.
- `rust/iroh-bridge/src/messages.rs`: Rust transport and local-API message
  types.
- `rust/iroh-bridge/src/state.rs`: Local queue state for outbound and inbound
  messages plus sidecar identity.
- `rust/iroh-bridge/src/http.rs`: Local sidecar HTTP API used by the Go master
  and Go agent.
- `rust/iroh-bridge/src/network.rs`: Iroh endpoint setup, accept loop, and
  outbound flush logic.
- `rust/iroh-bridge/src/main.rs`: Sidecar bootstrap that serves the local HTTP
  API and runs Iroh background loops.
- `rust/iroh-bridge/tests/http_smoke.rs`: Local sidecar API tests without live
  networking.
- `rust/iroh-bridge/tests/iroh_loopback.rs`: Rust loopback test proving desired
  state and observed status cross the Iroh transport.
- `README.md`: Short current-milestone section so the repo reflects the approved
  first slice.
- `.gitignore`: Exclude `rust/iroh-bridge/target/` so Rust build artifacts are
  not committed.
- `docs/architecture.md`: Updated architecture note matching the Go-plus-sidecar
  split.

### Task 1: Shared Types, Node Registry, And Persistent Master State

**Covers:** [S1], [S2], [S3], [S5], [S6], [S8], [S9]

**Files:**

- Create: `go.mod`
- Create: `internal/control/types.go`
- Create: `internal/master/state/store.go`
- Test: `internal/master/state/store_test.go`

- [ ] **Step 1: Write the failing store test**

```go
// go.mod
module github.com/luddite-dev/deploy

go 1.24.0
```

```go
// internal/master/state/store_test.go
package state

import (
	"path/filepath"
	"testing"

	"github.com/luddite-dev/deploy/internal/control"
)

func TestStorePersistsNodesDesiredAndObservedState(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")

	store, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}

	if err := store.UpsertNode(control.NodeRecord{
		NodeID:       "node-a",
		EndpointAddr: `{"node_id":"peer-a"}`,
		Connected:    true,
		LastSeen:     "2026-06-19T00:00:00Z",
	}); err != nil {
		t.Fatal(err)
	}

	desired, err := store.PutDesiredDeployment("node-a", control.DeploymentSpec{
		Name:        "web",
		ComposeYAML: "services:\n  web:\n    image: nginx:latest\n",
	})
	if err != nil {
		t.Fatal(err)
	}

	if err := store.PutObservedDeployment(control.ObservedDeployment{
		NodeID:         "node-a",
		Name:           "web",
		AppliedVersion: desired.Version,
		State:          control.ApplySucceeded,
	}); err != nil {
		t.Fatal(err)
	}

	reopened, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}

	nodes := reopened.ListNodes()
	if len(nodes) != 1 {
		t.Fatalf("nodes = %d, want 1", len(nodes))
	}
	if nodes[0].EndpointAddr != `{"node_id":"peer-a"}` {
		t.Fatalf("endpoint addr = %q", nodes[0].EndpointAddr)
	}

	view, ok := reopened.GetDeploymentStatus("node-a", "web")
	if !ok {
		t.Fatal("deployment status missing after reopen")
	}
	if view.Desired.Version != 1 {
		t.Fatalf("desired version = %d, want 1", view.Desired.Version)
	}
	if view.Observed == nil || view.Observed.AppliedVersion != 1 {
		t.Fatalf("observed version = %+v, want 1", view.Observed)
	}
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:
`go test ./internal/master/state -run TestStorePersistsNodesDesiredAndObservedState -count=1`

Expected: FAIL with an undefined symbol error such as `undefined: Open`.

- [ ] **Step 3: Write the minimal implementation**

```go
// internal/control/types.go
package control

type ApplyState string

const (
	ApplyPending   ApplyState = "pending"
	ApplySucceeded ApplyState = "succeeded"
	ApplyFailed    ApplyState = "failed"
)

type NodeRecord struct {
	NodeID       string `json:"node_id"`
	EndpointAddr string `json:"endpoint_addr"`
	Connected    bool   `json:"connected"`
	LastSeen     string `json:"last_seen,omitempty"`
}

type DeploymentSpec struct {
	Name        string `json:"name"`
	ComposeYAML string `json:"compose_yaml"`
}

type DesiredDeployment struct {
	NodeID  string         `json:"node_id"`
	Version int            `json:"version"`
	Spec    DeploymentSpec `json:"spec"`
	Deleted bool           `json:"deleted"`
}

type ObservedDeployment struct {
	NodeID         string     `json:"node_id"`
	Name           string     `json:"name"`
	AppliedVersion int        `json:"applied_version"`
	State          ApplyState `json:"state"`
	Message        string     `json:"message,omitempty"`
}

type DeploymentStatusView struct {
	Desired  DesiredDeployment  `json:"desired"`
	Observed *ObservedDeployment `json:"observed,omitempty"`
}
```

```go
// internal/master/state/store.go
package state

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"sort"
	"sync"

	"github.com/luddite-dev/deploy/internal/control"
)

type snapshot struct {
	Nodes    map[string]control.NodeRecord                    `json:"nodes"`
	Desired  map[string]map[string]control.DesiredDeployment `json:"desired"`
	Observed map[string]map[string]control.ObservedDeployment `json:"observed"`
}

type Store struct {
	path string
	mu   sync.Mutex
	data snapshot
}

func Open(path string) (*Store, error) {
	s := &Store{
		path: path,
		data: snapshot{
			Nodes:    map[string]control.NodeRecord{},
			Desired:  map[string]map[string]control.DesiredDeployment{},
			Observed: map[string]map[string]control.ObservedDeployment{},
		},
	}

	buf, err := os.ReadFile(path)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return s, nil
		}
		return nil, err
	}

	if err := json.Unmarshal(buf, &s.data); err != nil {
		return nil, err
	}
	if s.data.Nodes == nil {
		s.data.Nodes = map[string]control.NodeRecord{}
	}
	if s.data.Desired == nil {
		s.data.Desired = map[string]map[string]control.DesiredDeployment{}
	}
	if s.data.Observed == nil {
		s.data.Observed = map[string]map[string]control.ObservedDeployment{}
	}

	return s, nil
}

func (s *Store) UpsertNode(node control.NodeRecord) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.data.Nodes[node.NodeID] = node
	return s.flushLocked()
}

func (s *Store) ListNodes() []control.NodeRecord {
	s.mu.Lock()
	defer s.mu.Unlock()

	out := make([]control.NodeRecord, 0, len(s.data.Nodes))
	for _, node := range s.data.Nodes {
		out = append(out, node)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].NodeID < out[j].NodeID })
	return out
}

func (s *Store) GetNode(nodeID string) (control.NodeRecord, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	node, ok := s.data.Nodes[nodeID]
	return node, ok
}

func (s *Store) PutDesiredDeployment(nodeID string, spec control.DeploymentSpec) (control.DesiredDeployment, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	if s.data.Desired[nodeID] == nil {
		s.data.Desired[nodeID] = map[string]control.DesiredDeployment{}
	}
	current := s.data.Desired[nodeID][spec.Name]
	next := control.DesiredDeployment{
		NodeID:  nodeID,
		Version: current.Version + 1,
		Spec:    spec,
	}
	s.data.Desired[nodeID][spec.Name] = next

	if err := s.flushLocked(); err != nil {
		return control.DesiredDeployment{}, err
	}
	return next, nil
}

func (s *Store) DeleteDesiredDeployment(nodeID, name string) (control.DesiredDeployment, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	if s.data.Desired[nodeID] == nil {
		s.data.Desired[nodeID] = map[string]control.DesiredDeployment{}
	}
	current := s.data.Desired[nodeID][name]
	next := control.DesiredDeployment{
		NodeID:  nodeID,
		Version: current.Version + 1,
		Spec: control.DeploymentSpec{
			Name: name,
		},
		Deleted: true,
	}
	s.data.Desired[nodeID][name] = next

	if err := s.flushLocked(); err != nil {
		return control.DesiredDeployment{}, err
	}
	return next, nil
}

func (s *Store) PutObservedDeployment(obs control.ObservedDeployment) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	if s.data.Observed[obs.NodeID] == nil {
		s.data.Observed[obs.NodeID] = map[string]control.ObservedDeployment{}
	}
	s.data.Observed[obs.NodeID][obs.Name] = obs
	return s.flushLocked()
}

func (s *Store) GetDeploymentStatus(nodeID, name string) (control.DeploymentStatusView, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()

	desiredByNode := s.data.Desired[nodeID]
	if desiredByNode == nil {
		return control.DeploymentStatusView{}, false
	}
	desired, ok := desiredByNode[name]
	if !ok {
		return control.DeploymentStatusView{}, false
	}

	var observed *control.ObservedDeployment
	if observedByNode := s.data.Observed[nodeID]; observedByNode != nil {
		if current, ok := observedByNode[name]; ok {
			copy := current
			observed = &copy
		}
	}

	return control.DeploymentStatusView{Desired: desired, Observed: observed}, true
}

func (s *Store) flushLocked() error {
	if err := os.MkdirAll(filepath.Dir(s.path), 0o755); err != nil {
		return err
	}
	buf, err := json.MarshalIndent(s.data, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(s.path, buf, 0o644)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run:
`go test ./internal/master/state -run TestStorePersistsNodesDesiredAndObservedState -count=1`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add go.mod internal/control/types.go internal/master/state/store.go internal/master/state/store_test.go
git commit -m "feat: add persistent control-plane state store"
```

### Task 2: Master HTTP API For Node Registration And Deployment Intent

**Covers:** [S2], [S3], [S5], [S6], [S7], [S8], [S9]

**Files:**

- Create: `internal/master/httpapi/server.go`
- Test: `internal/master/httpapi/server_test.go`

- [ ] **Step 1: Write the failing API tests**

```go
// internal/master/httpapi/server_test.go
package httpapi

import (
	"context"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"

	"github.com/luddite-dev/deploy/internal/control"
	"github.com/luddite-dev/deploy/internal/master/state"
)

type fakePublisher struct {
	endpointAddr string
	published    []control.DesiredDeployment
}

func (f *fakePublisher) PublishDesired(_ context.Context, endpointAddr string, dep control.DesiredDeployment) error {
	f.endpointAddr = endpointAddr
	f.published = append(f.published, dep)
	return nil
}

func TestServerRegistersNodeAndListsNodes(t *testing.T) {
	store, err := state.Open(filepath.Join(t.TempDir(), "state.json"))
	if err != nil {
		t.Fatal(err)
	}

	h := New(store, &fakePublisher{}, `{"node_id":"master-sidecar"}`)

	registerReq := httptest.NewRequest(http.MethodPost, "/nodes/register", strings.NewReader(`{"node_id":"node-a","endpoint_addr":"{\"node_id\":\"agent-sidecar\"}"}`))
	registerRec := httptest.NewRecorder()
	h.ServeHTTP(registerRec, registerReq)
	if registerRec.Code != http.StatusAccepted {
		t.Fatalf("register status = %d, want %d", registerRec.Code, http.StatusAccepted)
	}
	if !strings.Contains(registerRec.Body.String(), `master-sidecar`) {
		t.Fatalf("register body = %s, want master-sidecar", registerRec.Body.String())
	}

	listReq := httptest.NewRequest(http.MethodGet, "/nodes", nil)
	listRec := httptest.NewRecorder()
	h.ServeHTTP(listRec, listReq)
	if listRec.Code != http.StatusOK {
		t.Fatalf("list status = %d, want %d", listRec.Code, http.StatusOK)
	}
	if !strings.Contains(listRec.Body.String(), `"node_id":"node-a"`) {
		t.Fatalf("list body = %s, want node-a", listRec.Body.String())
	}
}

func TestServerPublishesDesiredAndDeleteToRegisteredNode(t *testing.T) {
	store, err := state.Open(filepath.Join(t.TempDir(), "state.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := store.UpsertNode(control.NodeRecord{NodeID: "node-a", EndpointAddr: `{"node_id":"agent-sidecar"}`, Connected: true}); err != nil {
		t.Fatal(err)
	}

	publisher := &fakePublisher{}
	h := New(store, publisher, `{"node_id":"master-sidecar"}`)

	createReq := httptest.NewRequest(http.MethodPost, "/nodes/node-a/deployments/web", strings.NewReader(`{"compose_yaml":"services:\n  web:\n    image: nginx:latest\n"}`))
	createRec := httptest.NewRecorder()
	h.ServeHTTP(createRec, createReq)
	if createRec.Code != http.StatusAccepted {
		t.Fatalf("create status = %d, want %d", createRec.Code, http.StatusAccepted)
	}
	if publisher.endpointAddr != `{"node_id":"agent-sidecar"}` {
		t.Fatalf("endpoint addr = %q", publisher.endpointAddr)
	}
	if len(publisher.published) != 1 || publisher.published[0].Version != 1 {
		t.Fatalf("published = %+v, want version 1", publisher.published)
	}

	deleteReq := httptest.NewRequest(http.MethodDelete, "/nodes/node-a/deployments/web", nil)
	deleteRec := httptest.NewRecorder()
	h.ServeHTTP(deleteRec, deleteReq)
	if deleteRec.Code != http.StatusAccepted {
		t.Fatalf("delete status = %d, want %d", deleteRec.Code, http.StatusAccepted)
	}
	if len(publisher.published) != 2 || !publisher.published[1].Deleted {
		t.Fatalf("delete publish = %+v, want deleted version", publisher.published)
	}
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
`go test ./internal/master/httpapi -run 'TestServer(RegistersNodeAndListsNodes|PublishesDesiredAndDeleteToRegisteredNode)' -count=1`

Expected: FAIL with an undefined symbol error such as `undefined: New`.

- [ ] **Step 3: Write the minimal implementation**

```go
// internal/master/httpapi/server.go
package httpapi

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"

	"github.com/luddite-dev/deploy/internal/control"
	"github.com/luddite-dev/deploy/internal/master/state"
)

type Publisher interface {
	PublishDesired(context.Context, string, control.DesiredDeployment) error
}

type Server struct {
	store              *state.Store
	publisher          Publisher
	masterEndpointAddr string
	mux                *http.ServeMux
}

type registerNodeRequest struct {
	NodeID       string `json:"node_id"`
	EndpointAddr string `json:"endpoint_addr"`
}

type registerNodeResponse struct {
	MasterEndpointAddr string `json:"master_endpoint_addr"`
}

type putDeploymentRequest struct {
	ComposeYAML string `json:"compose_yaml"`
}

func New(store *state.Store, publisher Publisher, masterEndpointAddr string) http.Handler {
	s := &Server{
		store:              store,
		publisher:          publisher,
		masterEndpointAddr: masterEndpointAddr,
		mux:                http.NewServeMux(),
	}
	s.routes()
	return s.mux
}

func (s *Server) routes() {
	s.mux.HandleFunc("/nodes/register", s.handleRegisterNode)
	s.mux.HandleFunc("/nodes", s.handleListNodes)
	s.mux.HandleFunc("/nodes/", s.handleDeployment)
}

func (s *Server) handleRegisterNode(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		w.WriteHeader(http.StatusMethodNotAllowed)
		return
	}
	var req registerNodeRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	if err := s.store.UpsertNode(control.NodeRecord{
		NodeID:       req.NodeID,
		EndpointAddr: req.EndpointAddr,
		Connected:    true,
	}); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusAccepted)
	_ = json.NewEncoder(w).Encode(registerNodeResponse{MasterEndpointAddr: s.masterEndpointAddr})
}

func (s *Server) handleListNodes(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		w.WriteHeader(http.StatusMethodNotAllowed)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(s.store.ListNodes())
}

func (s *Server) handleDeployment(w http.ResponseWriter, r *http.Request) {
	parts := strings.Split(strings.Trim(r.URL.Path, "/"), "/")
	if len(parts) != 4 || parts[0] != "nodes" || parts[2] != "deployments" {
		http.NotFound(w, r)
		return
	}
	nodeID := parts[1]
	name := parts[3]

	switch r.Method {
	case http.MethodPost:
		node, ok := s.store.GetNode(nodeID)
		if !ok {
			http.NotFound(w, r)
			return
		}
		var req putDeploymentRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		dep, err := s.store.PutDesiredDeployment(nodeID, control.DeploymentSpec{Name: name, ComposeYAML: req.ComposeYAML})
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		if err := s.publisher.PublishDesired(r.Context(), node.EndpointAddr, dep); err != nil {
			http.Error(w, err.Error(), http.StatusBadGateway)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusAccepted)
		_ = json.NewEncoder(w).Encode(dep)
	case http.MethodDelete:
		node, ok := s.store.GetNode(nodeID)
		if !ok {
			http.NotFound(w, r)
			return
		}
		dep, err := s.store.DeleteDesiredDeployment(nodeID, name)
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		if err := s.publisher.PublishDesired(r.Context(), node.EndpointAddr, dep); err != nil {
			http.Error(w, err.Error(), http.StatusBadGateway)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusAccepted)
		_ = json.NewEncoder(w).Encode(dep)
	case http.MethodGet:
		view, ok := s.store.GetDeploymentStatus(nodeID, name)
		if !ok {
			http.NotFound(w, r)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(view)
	default:
		w.WriteHeader(http.StatusMethodNotAllowed)
	}
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
`go test ./internal/master/httpapi -run 'TestServer(RegistersNodeAndListsNodes|PublishesDesiredAndDeleteToRegisteredNode)' -count=1`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add internal/master/httpapi/server.go internal/master/httpapi/server_test.go
git commit -m "feat: add master control-plane API"
```

### Task 3: Agent Reconcile Logic For Apply And Delete

**Covers:** [S3], [S4], [S6], [S7], [S8], [S9]

**Files:**

- Create: `internal/agent/runtime/podman.go`
- Create: `internal/agent/reconcile/reconcile.go`
- Test: `internal/agent/reconcile/reconcile_test.go`

- [ ] **Step 1: Write the failing reconcile test**

```go
// internal/agent/reconcile/reconcile_test.go
package reconcile

import (
	"context"
	"testing"

	"github.com/luddite-dev/deploy/internal/control"
)

type fakeRunner struct {
	applyCalls  []string
	removeCalls []string
}

func (f *fakeRunner) Apply(_ context.Context, name, composePath string) error {
	f.applyCalls = append(f.applyCalls, name+":"+composePath)
	return nil
}

func (f *fakeRunner) Remove(_ context.Context, name, composePath string) error {
	f.removeCalls = append(f.removeCalls, name+":"+composePath)
	return nil
}

func TestServiceApplySkipsSameVersionAndHandlesDelete(t *testing.T) {
	runner := &fakeRunner{}
	svc := New(t.TempDir(), runner)

	deploy := control.DesiredDeployment{
		NodeID:  "node-a",
		Version: 2,
		Spec: control.DeploymentSpec{
			Name:        "web",
			ComposeYAML: "services:\n  web:\n    image: nginx:latest\n",
		},
	}

	if _, err := svc.Apply(context.Background(), deploy); err != nil {
		t.Fatal(err)
	}
	if _, err := svc.Apply(context.Background(), deploy); err != nil {
		t.Fatal(err)
	}

	deleted := deploy
	deleted.Version = 3
	deleted.Deleted = true
	obs, err := svc.Apply(context.Background(), deleted)
	if err != nil {
		t.Fatal(err)
	}

	if len(runner.applyCalls) != 1 {
		t.Fatalf("apply calls = %d, want 1", len(runner.applyCalls))
	}
	if len(runner.removeCalls) != 1 {
		t.Fatalf("remove calls = %d, want 1", len(runner.removeCalls))
	}
	if obs.State != control.ApplySucceeded || obs.AppliedVersion != 3 {
		t.Fatalf("delete observed = %+v, want succeeded version 3", obs)
	}
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:
`go test ./internal/agent/reconcile -run TestServiceApplySkipsSameVersionAndHandlesDelete -count=1`

Expected: FAIL with an undefined symbol error such as `undefined: New`.

- [ ] **Step 3: Write the minimal implementation**

```go
// internal/agent/runtime/podman.go
package runtime

import (
	"context"
	"os/exec"
)

type Runner interface {
	Apply(context.Context, string, string) error
	Remove(context.Context, string, string) error
}

type Podman struct{}

func (Podman) Apply(ctx context.Context, _ string, composePath string) error {
	cmd := exec.CommandContext(ctx, "podman", "compose", "-f", composePath, "up", "-d")
	return cmd.Run()
}

func (Podman) Remove(ctx context.Context, _ string, composePath string) error {
	cmd := exec.CommandContext(ctx, "podman", "compose", "-f", composePath, "down")
	return cmd.Run()
}
```

```go
// internal/agent/reconcile/reconcile.go
package reconcile

import (
	"context"
	"os"
	"path/filepath"
	"sync"

	"github.com/luddite-dev/deploy/internal/agent/runtime"
	"github.com/luddite-dev/deploy/internal/control"
)

type Service struct {
	root         string
	runner       runtime.Runner
	mu           sync.Mutex
	appliedByApp map[string]int
}

func New(root string, runner runtime.Runner) *Service {
	return &Service{
		root:         root,
		runner:       runner,
		appliedByApp: map[string]int{},
	}
}

func (s *Service) Apply(ctx context.Context, desired control.DesiredDeployment) (control.ObservedDeployment, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	composeDir := filepath.Join(s.root, desired.Spec.Name)
	composePath := filepath.Join(composeDir, "compose.yaml")
	if err := os.MkdirAll(composeDir, 0o755); err != nil {
		return control.ObservedDeployment{}, err
	}
	if err := os.WriteFile(composePath, []byte(desired.Spec.ComposeYAML), 0o644); err != nil {
		return control.ObservedDeployment{}, err
	}

	if desired.Deleted {
		if s.appliedByApp[desired.Spec.Name] != 0 {
			if err := s.runner.Remove(ctx, desired.Spec.Name, composePath); err != nil {
				return control.ObservedDeployment{}, err
			}
			delete(s.appliedByApp, desired.Spec.Name)
		}
		return control.ObservedDeployment{
			NodeID:         desired.NodeID,
			Name:           desired.Spec.Name,
			AppliedVersion: desired.Version,
			State:          control.ApplySucceeded,
		}, nil
	}

	if s.appliedByApp[desired.Spec.Name] != desired.Version {
		if err := s.runner.Apply(ctx, desired.Spec.Name, composePath); err != nil {
			return control.ObservedDeployment{
				NodeID:         desired.NodeID,
				Name:           desired.Spec.Name,
				AppliedVersion: desired.Version,
				State:          control.ApplyFailed,
				Message:        err.Error(),
			}, nil
		}
		s.appliedByApp[desired.Spec.Name] = desired.Version
	}

	return control.ObservedDeployment{
		NodeID:         desired.NodeID,
		Name:           desired.Spec.Name,
		AppliedVersion: desired.Version,
		State:          control.ApplySucceeded,
	}, nil
}
```

- [ ] **Step 4: Run test to verify it passes**

Run:
`go test ./internal/agent/reconcile -run TestServiceApplySkipsSameVersionAndHandlesDelete -count=1`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add internal/agent/runtime/podman.go internal/agent/reconcile/reconcile.go internal/agent/reconcile/reconcile_test.go
git commit -m "feat: add agent reconcile service"
```

### Task 4: Local Rust Sidecar API And Go Client Boundary

**Covers:** [S2], [S3], [S4], [S6], [S7], [S9]

**Files:**

- Create: `internal/sidecar/client/client.go`
- Create: `internal/sidecar/client/client_test.go`
- Create: `rust/iroh-bridge/Cargo.toml`
- Create: `rust/iroh-bridge/src/lib.rs`
- Create: `rust/iroh-bridge/src/messages.rs`
- Create: `rust/iroh-bridge/src/state.rs`
- Create: `rust/iroh-bridge/src/http.rs`
- Create: `rust/iroh-bridge/src/main.rs`
- Test: `rust/iroh-bridge/tests/http_smoke.rs`

- [ ] **Step 1: Write the failing boundary tests**

```go
// internal/sidecar/client/client_test.go
package client

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/luddite-dev/deploy/internal/control"
)

func TestClientIdentityAndPublishDesired(t *testing.T) {
	var got map[string]any
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/v1/identity":
			w.Header().Set("Content-Type", "application/json")
			_ = json.NewEncoder(w).Encode(map[string]string{"endpoint_addr_json": `{"node_id":"local-sidecar"}`})
		case "/v1/master/publish":
			if err := json.NewDecoder(r.Body).Decode(&got); err != nil {
				t.Fatal(err)
			}
			w.WriteHeader(http.StatusAccepted)
		default:
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	defer srv.Close()

	cli := New(srv.URL)
	identity, err := cli.Identity(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if identity != `{"node_id":"local-sidecar"}` {
		t.Fatalf("identity = %q", identity)
	}

	err = cli.PublishDesired(context.Background(), `{"node_id":"agent-sidecar"}`, control.DesiredDeployment{
		NodeID:  "node-a",
		Version: 1,
		Spec: control.DeploymentSpec{
			Name:        "web",
			ComposeYAML: "services: {}\n",
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	if got["endpoint_addr_json"] != `{"node_id":"agent-sidecar"}` {
		t.Fatalf("publish body = %+v", got)
	}
}
```

```rust
// rust/iroh-bridge/tests/http_smoke.rs
use axum::{body::Body, http::{Request, StatusCode}};
use tower::ServiceExt;

use iroh_bridge::{http::router, state::AppState};

#[tokio::test]
async fn publish_route_queues_outbound_message_and_identity_is_visible() {
    let state = AppState::new("{\"node_id\":\"local-sidecar\"}".to_string());
    let app = router(state.clone());

    let identity = Request::get("/v1/identity").body(Body::empty()).unwrap();
    let identity_res = app.clone().oneshot(identity).await.unwrap();
    assert_eq!(identity_res.status(), StatusCode::OK);

    let publish = Request::post("/v1/master/publish")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"endpoint_addr_json":"{\"node_id\":\"agent-sidecar\"}","deployment":{"node_id":"node-a","version":1,"spec":{"name":"web","compose_yaml":"services: {}\n"},"deleted":false}}"#))
        .unwrap();

    let publish_res = app.oneshot(publish).await.unwrap();
    assert_eq!(publish_res.status(), StatusCode::ACCEPTED);

    let queued = state.take_next_desired_outbound().await.expect("queued desired outbound");
    assert_eq!(queued.deployment.spec.name, "web");
    assert_eq!(queued.endpoint_addr_json, r#"{\"node_id\":\"agent-sidecar\"}"#);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
`go test ./internal/sidecar/client -run TestClientIdentityAndPublishDesired -count=1`

Expected: FAIL with an undefined symbol error such as `undefined: New`.

Run:
`cargo test --manifest-path rust/iroh-bridge/Cargo.toml publish_route_queues_outbound_message_and_identity_is_visible -- --exact`

Expected: FAIL because the Rust sidecar crate does not exist yet.

- [ ] **Step 3: Write the minimal implementation**

```go
// internal/sidecar/client/client.go
package client

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"

	"github.com/luddite-dev/deploy/internal/control"
)

type Client struct {
	baseURL string
	http    *http.Client
}

type identityResponse struct {
	EndpointAddrJSON string `json:"endpoint_addr_json"`
}

type desiredDispatch struct {
	EndpointAddrJSON string                    `json:"endpoint_addr_json"`
	Deployment       control.DesiredDeployment `json:"deployment"`
}

type observedDispatch struct {
	EndpointAddrJSON string                     `json:"endpoint_addr_json"`
	Deployment       control.ObservedDeployment `json:"deployment"`
}

func New(baseURL string) *Client {
	return &Client{baseURL: baseURL, http: http.DefaultClient}
}

func (c *Client) Identity(ctx context.Context) (string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.baseURL+"/v1/identity", nil)
	if err != nil {
		return "", err
	}
	res, err := c.http.Do(req)
	if err != nil {
		return "", err
	}
	defer res.Body.Close()
	var out identityResponse
	if err := json.NewDecoder(res.Body).Decode(&out); err != nil {
		return "", err
	}
	return out.EndpointAddrJSON, nil
}

func (c *Client) PublishDesired(ctx context.Context, endpointAddr string, dep control.DesiredDeployment) error {
	return c.post(ctx, "/v1/master/publish", desiredDispatch{EndpointAddrJSON: endpointAddr, Deployment: dep})
}

func (c *Client) ReportObserved(ctx context.Context, endpointAddr string, obs control.ObservedDeployment) error {
	return c.post(ctx, "/v1/agent/report", observedDispatch{EndpointAddrJSON: endpointAddr, Deployment: obs})
}

func (c *Client) PollDesired(ctx context.Context) ([]control.DesiredDeployment, error) {
	return getSlice[control.DesiredDeployment](ctx, c.http, c.baseURL+"/v1/agent/messages")
}

func (c *Client) PollObserved(ctx context.Context) ([]control.ObservedDeployment, error) {
	return getSlice[control.ObservedDeployment](ctx, c.http, c.baseURL+"/v1/master/reports")
}

func (c *Client) post(ctx context.Context, path string, body any) error {
	buf, err := json.Marshal(body)
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.baseURL+path, bytes.NewReader(buf))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	res, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	if res.StatusCode >= 300 {
		return fmt.Errorf("sidecar status %d", res.StatusCode)
	}
	return nil
}

func getSlice[T any](ctx context.Context, client *http.Client, url string) ([]T, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	res, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer res.Body.Close()
	var out []T
	if err := json.NewDecoder(res.Body).Decode(&out); err != nil {
		return nil, err
	}
	return out, nil
}
```

```rust
// rust/iroh-bridge/Cargo.toml
[package]
name = "iroh-bridge"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1"
axum = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "sync", "net", "time"] }
tower = "0.5"
```

```rust
// rust/iroh-bridge/src/lib.rs
pub mod http;
pub mod messages;
pub mod state;
```

```rust
// rust/iroh-bridge/src/messages.rs
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeploymentSpec {
    pub name: String,
    pub compose_yaml: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesiredDeployment {
    pub node_id: String,
    pub version: u64,
    pub spec: DeploymentSpec,
    pub deleted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObservedDeployment {
    pub node_id: String,
    pub name: String,
    pub applied_version: u64,
    pub state: String,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesiredDispatch {
    pub endpoint_addr_json: String,
    pub deployment: DesiredDeployment,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObservedDispatch {
    pub endpoint_addr_json: String,
    pub deployment: ObservedDeployment,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityResponse {
    pub endpoint_addr_json: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Envelope {
    Desired { deployment: DesiredDeployment },
    Observed { deployment: ObservedDeployment },
}
```

```rust
// rust/iroh-bridge/src/state.rs
use std::{collections::VecDeque, sync::Arc};

use tokio::sync::Mutex;

use crate::messages::{DesiredDeployment, DesiredDispatch, ObservedDeployment, ObservedDispatch};

#[derive(Clone)]
pub struct AppState {
    identity: Arc<Mutex<String>>,
    desired_outbound: Arc<Mutex<VecDeque<DesiredDispatch>>>,
    desired_inbound: Arc<Mutex<VecDeque<DesiredDeployment>>>,
    observed_outbound: Arc<Mutex<VecDeque<ObservedDispatch>>>,
    observed_inbound: Arc<Mutex<VecDeque<ObservedDeployment>>>,
}

impl AppState {
    pub fn new(identity: String) -> Self {
        Self {
            identity: Arc::new(Mutex::new(identity)),
            desired_outbound: Arc::new(Mutex::new(VecDeque::new())),
            desired_inbound: Arc::new(Mutex::new(VecDeque::new())),
            observed_outbound: Arc::new(Mutex::new(VecDeque::new())),
            observed_inbound: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub async fn identity(&self) -> String {
        self.identity.lock().await.clone()
    }

    pub async fn set_identity(&self, identity: String) {
        *self.identity.lock().await = identity;
    }

    pub async fn push_desired_outbound(&self, dispatch: DesiredDispatch) {
        self.desired_outbound.lock().await.push_back(dispatch);
    }

    pub async fn take_next_desired_outbound(&self) -> Option<DesiredDispatch> {
        self.desired_outbound.lock().await.pop_front()
    }

    pub async fn push_desired_inbound(&self, deployment: DesiredDeployment) {
        self.desired_inbound.lock().await.push_back(deployment);
    }

    pub async fn take_desired_inbound(&self) -> Vec<DesiredDeployment> {
        self.desired_inbound.lock().await.drain(..).collect()
    }

    pub async fn push_observed_outbound(&self, dispatch: ObservedDispatch) {
        self.observed_outbound.lock().await.push_back(dispatch);
    }

    pub async fn take_next_observed_outbound(&self) -> Option<ObservedDispatch> {
        self.observed_outbound.lock().await.pop_front()
    }

    pub async fn push_observed_inbound(&self, deployment: ObservedDeployment) {
        self.observed_inbound.lock().await.push_back(deployment);
    }

    pub async fn take_observed_inbound(&self) -> Vec<ObservedDeployment> {
        self.observed_inbound.lock().await.drain(..).collect()
    }
}
```

```rust
// rust/iroh-bridge/src/http.rs
use axum::{extract::State, http::StatusCode, routing::{get, post}, Json, Router};

use crate::{
    messages::{DesiredDeployment, DesiredDispatch, IdentityResponse, ObservedDeployment, ObservedDispatch},
    state::AppState,
};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/identity", get(identity))
        .route("/v1/master/publish", post(queue_desired))
        .route("/v1/master/reports", get(take_observed))
        .route("/v1/agent/messages", get(take_desired))
        .route("/v1/agent/report", post(queue_observed))
        .with_state(state)
}

async fn identity(State(state): State<AppState>) -> Json<IdentityResponse> {
    Json(IdentityResponse {
        endpoint_addr_json: state.identity().await,
    })
}

async fn queue_desired(State(state): State<AppState>, Json(dispatch): Json<DesiredDispatch>) -> (StatusCode, Json<DesiredDispatch>) {
    state.push_desired_outbound(dispatch.clone()).await;
    (StatusCode::ACCEPTED, Json(dispatch))
}

async fn take_desired(State(state): State<AppState>) -> Json<Vec<DesiredDeployment>> {
    Json(state.take_desired_inbound().await)
}

async fn queue_observed(State(state): State<AppState>, Json(dispatch): Json<ObservedDispatch>) -> (StatusCode, Json<ObservedDispatch>) {
    state.push_observed_outbound(dispatch.clone()).await;
    (StatusCode::ACCEPTED, Json(dispatch))
}

async fn take_observed(State(state): State<AppState>) -> Json<Vec<ObservedDeployment>> {
    Json(state.take_observed_inbound().await)
}
```

```rust
// rust/iroh-bridge/src/main.rs
use std::net::SocketAddr;

use anyhow::Result;
use tokio::net::TcpListener;

use iroh_bridge::{http::router, state::AppState};

#[tokio::main]
async fn main() -> Result<()> {
    let bind_addr: SocketAddr = std::env::var("LUDDITE_SIDECAR_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:7777".to_string())
        .parse()?;
    let identity = std::env::var("LUDDITE_ENDPOINT_ADDR_JSON").unwrap_or_default();
    let state = AppState::new(identity);

    let listener = TcpListener::bind(bind_addr).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
`go test ./internal/sidecar/client -run TestClientIdentityAndPublishDesired -count=1`

Expected: PASS.

Run:
`cargo test --manifest-path rust/iroh-bridge/Cargo.toml publish_route_queues_outbound_message_and_identity_is_visible -- --exact`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add .gitignore internal/sidecar/client/client.go internal/sidecar/client/client_test.go rust/iroh-bridge
git commit -m "feat: add local sidecar boundary"
```

### Task 5: Add Iroh Transport And Wire The First End-To-End Flow

**Covers:** [S3], [S4], [S5], [S6], [S7], [S8], [S9], [S10]

**Files:**

- Create: `cmd/luddite-master/main.go`
- Create: `cmd/luddite-agent/main.go`
- Create: `rust/iroh-bridge/src/network.rs`
- Test: `rust/iroh-bridge/tests/iroh_loopback.rs`
- Modify: `rust/iroh-bridge/Cargo.toml`
- Modify: `rust/iroh-bridge/src/lib.rs`
- Modify: `rust/iroh-bridge/src/main.rs`
- Modify: `README.md`
- Modify: `docs/architecture.md`

- [ ] **Step 1: Write the failing Iroh loopback test**

```rust
// rust/iroh-bridge/tests/iroh_loopback.rs
use std::time::Duration;

use iroh_bridge::{
    messages::{DeploymentSpec, DesiredDeployment, DesiredDispatch, ObservedDeployment, ObservedDispatch},
    network::Network,
    state::AppState,
};
use tokio::time::timeout;

#[tokio::test]
async fn desired_state_and_observed_status_cross_the_iroh_transport() {
    let master_state = AppState::new(String::new());
    let agent_state = AppState::new(String::new());

    let master = Network::bind(master_state.clone()).await.unwrap();
    let agent = Network::bind(agent_state.clone()).await.unwrap();

    timeout(Duration::from_secs(30), master.refresh_identity())
        .await
        .expect("master refresh should not hang")
        .unwrap();
    timeout(Duration::from_secs(30), agent.refresh_identity())
        .await
        .expect("agent refresh should not hang")
        .unwrap();

    master_state.push_desired_outbound(DesiredDispatch {
        endpoint_addr_json: agent_state.identity().await,
        deployment: DesiredDeployment {
            node_id: "node-a".into(),
            version: 1,
            spec: DeploymentSpec {
                name: "web".into(),
                compose_yaml: "services:\n  web:\n    image: nginx:latest\n".into(),
            },
            deleted: false,
        },
    }).await;

    timeout(Duration::from_secs(30), master.flush_outbound_once())
        .await
        .expect("master flush should not hang")
        .unwrap();

    let desired = agent_state.take_desired_inbound().await;
    assert_eq!(desired.len(), 1);
    assert_eq!(desired[0].spec.name, "web");

    agent_state.push_observed_outbound(ObservedDispatch {
        endpoint_addr_json: master_state.identity().await,
        deployment: ObservedDeployment {
            node_id: "node-a".into(),
            name: "web".into(),
            applied_version: 1,
            state: "succeeded".into(),
            message: None,
        },
    }).await;

    timeout(Duration::from_secs(30), agent.flush_outbound_once())
        .await
        .expect("agent flush should not hang")
        .unwrap();

    let observed = master_state.take_observed_inbound().await;
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].name, "web");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:
`cargo test --manifest-path rust/iroh-bridge/Cargo.toml desired_state_and_observed_status_cross_the_iroh_transport -- --exact`

Expected: FAIL with an undefined module or symbol error such as
`could not find network in iroh_bridge`.

- [ ] **Step 3: Write the minimal transport and wiring implementation**

```rust
// rust/iroh-bridge/Cargo.toml
[package]
name = "iroh-bridge"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1"
axum = "0.8"
iroh = "1.0"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "sync", "net", "time"] }
tower = "0.5"
```

```rust
// rust/iroh-bridge/src/lib.rs
pub mod http;
pub mod messages;
pub mod network;
pub mod state;
```

```rust
// rust/iroh-bridge/src/network.rs
use anyhow::{anyhow, Result};
use iroh::{endpoint::presets, Endpoint, EndpointAddr};

use crate::{messages::{DesiredDispatch, Envelope, ObservedDispatch}, state::AppState};

const ALPN: &[u8] = b"luddite/control/1";
const ACK: &[u8] = b"ok";

#[derive(Clone)]
pub struct Network {
    endpoint: Endpoint,
    state: AppState,
}

impl Network {
    pub async fn bind(state: AppState) -> Result<Self> {
        let endpoint = Endpoint::builder(presets::N0)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await?;

        let network = Self {
            endpoint: endpoint.clone(),
            state: state.clone(),
        };

        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let state = state.clone();
                tokio::spawn(async move {
                    if let Ok(connecting) = incoming.await {
                        let _ = handle_connection(state, connecting).await;
                    }
                });
            }
        });

        Ok(network)
    }

    pub async fn refresh_identity(&self) -> Result<()> {
        // Wait for the endpoint to discover its relay URL; after this, addr()
        // includes both direct addresses and the relay address.
        self.endpoint.online().await;
        let addr = self.endpoint.addr();
        self.state.set_identity(serde_json::to_string(&addr)?).await;
        Ok(())
    }

    pub async fn flush_outbound_once(&self) -> Result<()> {
        if let Some(dispatch) = self.state.take_next_desired_outbound().await {
            self.send(dispatch.endpoint_addr_json.as_str(), Envelope::Desired { deployment: dispatch.deployment }).await?;
        }
        if let Some(dispatch) = self.state.take_next_observed_outbound().await {
            self.send(dispatch.endpoint_addr_json.as_str(), Envelope::Observed { deployment: dispatch.deployment }).await?;
        }
        Ok(())
    }

    async fn send(&self, endpoint_addr_json: &str, envelope: Envelope) -> Result<()> {
        let addr: EndpointAddr = serde_json::from_str(endpoint_addr_json)?;
        let conn = self.endpoint.connect(addr, ALPN).await?;
        let (mut send, mut recv) = conn.open_bi().await?;
        send.write_all(&serde_json::to_vec(&envelope)?).await?;
        send.finish()?;
        let ack = recv.read_to_end(32).await?;
        if ack != ACK {
            return Err(anyhow!("unexpected ack: {:?}", ack));
        }
        conn.close(0u32.into(), b"done");
        Ok(())
    }
}

async fn handle_connection(state: AppState, connection: iroh::endpoint::Connection) -> Result<()> {
    let (mut send, mut recv) = connection.accept_bi().await?;
    let payload = recv.read_to_end(1 << 20).await?;
    let envelope: Envelope = serde_json::from_slice(&payload)?;

    match envelope {
        Envelope::Desired { deployment } => state.push_desired_inbound(deployment).await,
        Envelope::Observed { deployment } => state.push_observed_inbound(deployment).await,
    }

    send.write_all(ACK).await?;
    send.finish()?;
    connection.closed().await;
    Ok(())
}
```

```rust
// rust/iroh-bridge/src/main.rs
use std::{net::SocketAddr, time::Duration};

use anyhow::Result;
use tokio::net::TcpListener;

use iroh_bridge::{http::router, network::Network, state::AppState};

#[tokio::main]
async fn main() -> Result<()> {
    let bind_addr: SocketAddr = std::env::var("LUDDITE_SIDECAR_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:7777".to_string())
        .parse()?;
    let state = AppState::new(String::new());
    let network = Network::bind(state.clone()).await?;
    network.refresh_identity().await?;

    tokio::spawn({
        let network = network.clone();
        async move {
            loop {
                let _ = network.flush_outbound_once().await;
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    });

    let listener = TcpListener::bind(bind_addr).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}
```

```go
// cmd/luddite-master/main.go
package main

import (
	"context"
	"log"
	"net/http"
	"os"
	"time"

	masterhttp "github.com/luddite-dev/deploy/internal/master/httpapi"
	"github.com/luddite-dev/deploy/internal/master/state"
	"github.com/luddite-dev/deploy/internal/sidecar/client"
)

func main() {
	store, err := state.Open(os.Getenv("LUDDITE_MASTER_STATE"))
	if err != nil {
		log.Fatal(err)
	}

	sidecar := client.New(os.Getenv("LUDDITE_MASTER_SIDECAR"))
	masterEndpointAddr, err := sidecar.Identity(context.Background())
	if err != nil {
		log.Fatal(err)
	}

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

```go
// cmd/luddite-agent/main.go
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"log"
	"net/http"
	"os"
	"time"

	"github.com/luddite-dev/deploy/internal/agent/reconcile"
	"github.com/luddite-dev/deploy/internal/agent/runtime"
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
	sidecar := client.New(os.Getenv("LUDDITE_AGENT_SIDECAR"))
	reconciler := reconcile.New(os.Getenv("LUDDITE_AGENT_ROOT"), runtime.Podman{})
	nodeID := os.Getenv("LUDDITE_NODE_ID")
	masterAPI := os.Getenv("LUDDITE_MASTER_API")

	agentEndpointAddr, err := sidecar.Identity(context.Background())
	if err != nil {
		log.Fatal(err)
	}
	masterEndpointAddr, err := registerWithMaster(masterAPI, nodeID, agentEndpointAddr)
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
	var out registerNodeResponse
	if err := json.NewDecoder(res.Body).Decode(&out); err != nil {
		return "", err
	}
	return out.MasterEndpointAddr, nil
}
```

```md
<!-- README.md -->

## Current Milestone

The first implementation milestone is the remote control plane only.

- Go master: node registration, desired-state persistence, operator HTTP API
- Go agent: Podman reconcile loop for node-scoped deployments
- Rust `iroh-bridge`: local sidecar that moves desired state and observed status
  over Iroh

Persistent storage, backups, DNS, HTTPS, and rollback semantics remain out of
scope for this milestone.
```

```md
<!-- docs/architecture.md -->

# Architecture

The first milestone uses three runtime pieces:

- a Go master service for operator HTTP requests, node records, and
  desired-state persistence
- a Go agent service for local Podman reconciliation on each node
- a Rust `iroh-bridge` sidecar on both the master and agent machines for Iroh
  connectivity

Nodes still connect outward from constrained networks, but the application logic
stays almost entirely in Go because the Iroh-specific work is isolated behind
the sidecar boundary.
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
`cargo test --manifest-path rust/iroh-bridge/Cargo.toml desired_state_and_observed_status_cross_the_iroh_transport -- --exact`

Expected: PASS.

> The loopback test reaches the n0 relay while waiting for `online()`. If the
> relay is unreachable, the test can be made hermetic by binding
> `127.0.0.1:0` and constructing the dial address as
> `EndpointAddr::new(endpoint.id()).with_ip_addr(endpoint.bound_sockets()[0])`.

- [ ] **Step 5: Commit**

```bash
git add .gitignore cmd/luddite-master/main.go cmd/luddite-agent/main.go rust/iroh-bridge/Cargo.toml rust/iroh-bridge/src/lib.rs rust/iroh-bridge/src/network.rs rust/iroh-bridge/src/main.rs rust/iroh-bridge/tests/iroh_loopback.rs README.md docs/architecture.md
git commit -m "feat: wire Iroh sidecars into the control plane"
```
