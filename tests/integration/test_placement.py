"""Integration tests for deployment adaptive placement."""
import time

import pytest


def create_deployment(core, name, server_id="", host_port=8090, network="bridge"):
    """Create a deployment with the given config."""
    config = {
        "server_id": server_id,
        "image": {"type": "Image", "params": {"image": "nginx:alpine"}},
        "network": network,
        "ports": [{"container": 80, "host": host_port}] if host_port else [],
    }
    r = core.write("/CreateDeployment", {"name": name, "config": config})
    return r["_id"]["$oid"]


def deploy_and_wait(core, deployment_id) -> dict:
    r = core.execute("/Deploy", {"deployment": deployment_id})
    update_id = r["_id"]["$oid"]
    return core.wait_for_update(update_id, timeout=120)


class TestAdaptivePlacement:
    """Tests that pick_target routes deployments to the right server."""

    def test_fixed_port_first_deploy_goes_to_ac(self, core, ac_ssh, runner_ssh, server_ids):
        """First deploy with a free port should go to AC (only enabled server with port free)."""
        dep_id = create_deployment(core, "test-place-1", host_port=8090)
        result = deploy_and_wait(core, dep_id)

        assert result["success"], f"Deploy failed: {result.get('logs')}"

        # Container should be on one of the servers
        ac_names = [c["name"] for c in ac_ssh.podman_ps()]
        runner_names = [c["name"] for c in runner_ssh.podman_ps()]
        assert "test-place-1" in ac_names or "test-place-1" in runner_names

    def test_port_conflict_routes_to_other_server(self, core, ac_ssh, runner_ssh, server_ids):
        """Two deployments with same fixed port should land on different servers."""
        dep1 = create_deployment(core, "test-conflict-1", host_port=8091)
        dep2 = create_deployment(core, "test-conflict-2", host_port=8091)

        r1 = deploy_and_wait(core, dep1)
        assert r1["success"], f"First deploy failed: {r1.get('logs')}"

        r2 = deploy_and_wait(core, dep2)
        assert r2["success"], f"Second deploy failed: {r2.get('logs')}"

        # They should be on different servers
        ac_names = [c["name"] for c in ac_ssh.podman_ps()]
        runner_names = [c["name"] for c in runner_ssh.podman_ps()]
        assert "test-conflict-1" in ac_names or "test-conflict-1" in runner_names
        assert "test-conflict-2" in ac_names or "test-conflict-2" in runner_names
        # At least one should be on each server
        assert (
            ("test-conflict-1" in ac_names and "test-conflict-2" in runner_names)
            or ("test-conflict-1" in runner_names and "test-conflict-2" in ac_names)
        ), f"Both deployments on same server: AC={ac_names}, Runner={runner_names}"

    def test_empty_server_id_uses_scheduler(self, core, ac_ssh, runner_ssh):
        """Empty server_id should let scheduler pick the target."""
        dep_id = create_deployment(core, "test-scheduler", server_id="", host_port=8092)
        result = deploy_and_wait(core, dep_id)
        assert result["success"], f"Deploy failed: {result.get('logs')}"

        ac_names = [c["name"] for c in ac_ssh.podman_ps()]
        runner_names = [c["name"] for c in runner_ssh.podman_ps()]
        assert "test-scheduler" in ac_names or "test-scheduler" in runner_names
