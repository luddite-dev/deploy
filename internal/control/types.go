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
	Desired  DesiredDeployment   `json:"desired"`
	Observed *ObservedDeployment `json:"observed,omitempty"`
}
