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
