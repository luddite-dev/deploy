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
