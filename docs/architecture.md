# Architecture

For simplicity, we design around a master server which slave nodes can connect
to. Slave nodes allow the master server to control podman containers and storage
volumes on the machine.
