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
		// If publish fails after persist, a retry produces a new version with the same spec; this is intentional — the agent converges to the latest version.
		dep, err := s.store.PutDesiredDeployment(nodeID, control.DeploymentSpec{Name: name, ComposeYAML: req.ComposeYAML})
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		obs := control.ObservedDeployment{
			NodeID:         nodeID,
			Name:           name,
			AppliedVersion: dep.Version,
			State:          control.ApplyPending,
		}
		_ = s.store.PutObservedDeployment(obs)
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
		obs := control.ObservedDeployment{
			NodeID:         nodeID,
			Name:           name,
			AppliedVersion: dep.Version,
			State:          control.ApplyPending,
		}
		_ = s.store.PutObservedDeployment(obs)
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
