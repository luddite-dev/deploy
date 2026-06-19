package main

import (
	"context"
	"flag"
	"log"
	"net/http"
	"time"

	"github.com/luddite-dev/deploy/internal/envutil"
	masterhttp "github.com/luddite-dev/deploy/internal/master/httpapi"
	"github.com/luddite-dev/deploy/internal/master/state"
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
