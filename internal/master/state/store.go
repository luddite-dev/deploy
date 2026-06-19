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
	Desired  map[string]map[string]control.DesiredDeployment  `json:"desired"`
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
