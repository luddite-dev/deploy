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
