# Luddite Deploy

> [!NOTE] This project is still a work in progress in terms of scoping and
> design, and thus subject to change. There are no stable APIs until v1.

Kubernetes is just a tad too complicated, podman itself a bit too limited.

The goal of this project is to simplify small self-hosted deployments with
built-in support for persistent storage, backups, DNS, HTTPS, and port-based
allocation.

It shall be a wrapper around podman, and strive to use extend standards where
possible, rather than creating things from scratch. For example, using compose
for multiple services in one deployment.
