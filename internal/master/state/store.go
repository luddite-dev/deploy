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

	next := cloneSnapshot(s.data)
	next.Nodes[node.NodeID] = node
	if err := s.flushLocked(next); err != nil {
		return err
	}
	s.data = next
	return nil
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

	updated := cloneSnapshot(s.data)
	if updated.Desired[nodeID] == nil {
		updated.Desired[nodeID] = map[string]control.DesiredDeployment{}
	}
	current := updated.Desired[nodeID][spec.Name]
	next := control.DesiredDeployment{
		NodeID:  nodeID,
		Version: current.Version + 1,
		Spec:    spec,
	}
	updated.Desired[nodeID][spec.Name] = next

	if err := s.flushLocked(updated); err != nil {
		return control.DesiredDeployment{}, err
	}
	s.data = updated

	return next, nil
}

func (s *Store) DeleteDesiredDeployment(nodeID, name string) (control.DesiredDeployment, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	updated := cloneSnapshot(s.data)
	if updated.Desired[nodeID] == nil {
		updated.Desired[nodeID] = map[string]control.DesiredDeployment{}
	}
	current := updated.Desired[nodeID][name]
	next := control.DesiredDeployment{
		NodeID:  nodeID,
		Version: current.Version + 1,
		Spec: control.DeploymentSpec{
			Name: name,
		},
		Deleted: true,
	}
	updated.Desired[nodeID][name] = next

	if err := s.flushLocked(updated); err != nil {
		return control.DesiredDeployment{}, err
	}
	s.data = updated

	return next, nil
}

func (s *Store) PutObservedDeployment(obs control.ObservedDeployment) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	next := cloneSnapshot(s.data)
	if next.Observed[obs.NodeID] == nil {
		next.Observed[obs.NodeID] = map[string]control.ObservedDeployment{}
	}
	next.Observed[obs.NodeID][obs.Name] = obs
	if err := s.flushLocked(next); err != nil {
		return err
	}
	s.data = next
	return nil
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

func (s *Store) flushLocked(data snapshot) error {
	if err := os.MkdirAll(filepath.Dir(s.path), 0o755); err != nil {
		return err
	}

	buf, err := json.MarshalIndent(data, "", "  ")
	if err != nil {
		return err
	}

	tmp, err := os.CreateTemp(filepath.Dir(s.path), filepath.Base(s.path)+".tmp-*")
	if err != nil {
		return err
	}
	tmpPath := tmp.Name()
	defer os.Remove(tmpPath)

	if _, err := tmp.Write(buf); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Chmod(0o644); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Sync(); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}

	return os.Rename(tmpPath, s.path)
}

func cloneSnapshot(src snapshot) snapshot {
	return snapshot{
		Nodes:    cloneNodes(src.Nodes),
		Desired:  cloneDesired(src.Desired),
		Observed: cloneObserved(src.Observed),
	}
}

func cloneNodes(src map[string]control.NodeRecord) map[string]control.NodeRecord {
	dst := make(map[string]control.NodeRecord, len(src))
	for key, value := range src {
		dst[key] = value
	}
	return dst
}

func cloneDesired(src map[string]map[string]control.DesiredDeployment) map[string]map[string]control.DesiredDeployment {
	dst := make(map[string]map[string]control.DesiredDeployment, len(src))
	for nodeID, deployments := range src {
		copy := make(map[string]control.DesiredDeployment, len(deployments))
		for name, deployment := range deployments {
			copy[name] = deployment
		}
		dst[nodeID] = copy
	}
	return dst
}

func cloneObserved(src map[string]map[string]control.ObservedDeployment) map[string]map[string]control.ObservedDeployment {
	dst := make(map[string]map[string]control.ObservedDeployment, len(src))
	for nodeID, deployments := range src {
		copy := make(map[string]control.ObservedDeployment, len(deployments))
		for name, deployment := range deployments {
			copy[name] = deployment
		}
		dst[nodeID] = copy
	}
	return dst
}
