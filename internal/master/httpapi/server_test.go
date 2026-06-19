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

func TestServerGetStatusReturnsDesiredAndObservedView(t *testing.T) {
	store, err := state.Open(filepath.Join(t.TempDir(), "state.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := store.UpsertNode(control.NodeRecord{NodeID: "node-a", EndpointAddr: `{"node_id":"agent-sidecar"}`, Connected: true}); err != nil {
		t.Fatal(err)
	}

	h := New(store, &fakePublisher{}, `{"node_id":"master-sidecar"}`)

	createReq := httptest.NewRequest(http.MethodPost, "/nodes/node-a/deployments/web", strings.NewReader(`{"compose_yaml":"services:\n  web:\n    image: nginx:latest\n"}`))
	createRec := httptest.NewRecorder()
	h.ServeHTTP(createRec, createReq)
	if createRec.Code != http.StatusAccepted {
		t.Fatalf("create status = %d, want %d", createRec.Code, http.StatusAccepted)
	}

	getReq := httptest.NewRequest(http.MethodGet, "/nodes/node-a/deployments/web", nil)
	getRec := httptest.NewRecorder()
	h.ServeHTTP(getRec, getReq)
	if getRec.Code != http.StatusOK {
		t.Fatalf("get status = %d, want %d", getRec.Code, http.StatusOK)
	}
	body := getRec.Body.String()
	if !strings.Contains(body, `"version":1`) {
		t.Fatalf("get body = %s, want desired.version == 1", body)
	}
	if !strings.Contains(body, `"state":"pending"`) {
		t.Fatalf("get body = %s, want observed.state == pending", body)
	}
}

func TestServerPostUnknownNodeReturns404(t *testing.T) {
	store, err := state.Open(filepath.Join(t.TempDir(), "state.json"))
	if err != nil {
		t.Fatal(err)
	}

	h := New(store, &fakePublisher{}, `{"node_id":"master-sidecar"}`)

	req := httptest.NewRequest(http.MethodPost, "/nodes/ghost/deployments/web", strings.NewReader(`{"compose_yaml":"x"}`))
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	if rec.Code != http.StatusNotFound {
		t.Fatalf("status = %d, want %d", rec.Code, http.StatusNotFound)
	}
}

func TestServerPostMalformedJSONReturns400(t *testing.T) {
	store, err := state.Open(filepath.Join(t.TempDir(), "state.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := store.UpsertNode(control.NodeRecord{NodeID: "node-a", EndpointAddr: `{"node_id":"agent-sidecar"}`, Connected: true}); err != nil {
		t.Fatal(err)
	}

	h := New(store, &fakePublisher{}, `{"node_id":"master-sidecar"}`)

	req := httptest.NewRequest(http.MethodPost, "/nodes/node-a/deployments/web", strings.NewReader(`not json`))
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want %d", rec.Code, http.StatusBadRequest)
	}
}

func TestServerWrongMethodReturns405(t *testing.T) {
	store, err := state.Open(filepath.Join(t.TempDir(), "state.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := store.UpsertNode(control.NodeRecord{NodeID: "node-a", EndpointAddr: `{"node_id":"agent-sidecar"}`, Connected: true}); err != nil {
		t.Fatal(err)
	}

	h := New(store, &fakePublisher{}, `{"node_id":"master-sidecar"}`)

	req := httptest.NewRequest(http.MethodPut, "/nodes/node-a/deployments/web", nil)
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	if rec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("status = %d, want %d", rec.Code, http.StatusMethodNotAllowed)
	}
}
