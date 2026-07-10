"""Integration tests for stack deploy and migration."""
import time

import pytest


COMPOSE_NGINX = """\
services:
  web:
    image: nginx:alpine
    ports:
      - "{host_port}:80"
    volumes:
      - {volume_name}:/usr/share/nginx/html
volumes:
  {volume_name}:
"""


def create_stack(core, name, host_port, server_id=""):
    """Create a UI-defined stack with a simple nginx service."""
    compose_contents = COMPOSE_NGINX.format(
        host_port=host_port,
        volume_name=f"{name}-vol",
    )
    config = {
        "server_id": server_id,
        "project_name": name,
        "file_paths": ["compose.yml"],
        "file_contents": compose_contents,
    }
    r = core.write("/CreateStack", {"name": name, "config": config})
    return r["_id"]["$oid"]


def deploy_stack(core, stack_id) -> dict:
    r = core.execute("/DeployStack", {"stack": stack_id})
    update_id = r["_id"]["$oid"]
    return core.wait_for_update(update_id, timeout=120)


def drain_server(core, server_id) -> dict:
    """Set desired_state to Drain. Must send full config."""
    servers = core.list_servers()
    for s in servers:
        if s["_id"]["$oid"] == server_id:
            config = s["config"]
            config["desired_state"] = "Drain"
            return core.update_server(server_id, config)
    raise ValueError(f"Server {server_id} not found")


def undrain_server(core, server_id) -> dict:
    """Set desired_state back to Run. Must send full config."""
    servers = core.list_servers()
    for s in servers:
        if s["_id"]["$oid"] == server_id:
            config = s["config"]
            config["desired_state"] = "Run"
            return core.update_server(server_id, config)
    raise ValueError(f"Server {server_id} not found")


def wait_for_server_state(core, server_id, target_state, timeout=300):
    for _ in range(timeout // 3):
        servers = core.list_servers()
        for s in servers:
            if s["_id"]["$oid"] == server_id:
                if s["info"]["state"] == target_state:
                    return True
        time.sleep(3)
    return False


def get_stack_info(core, stack_id) -> dict:
    r = core.read("/ListFullStacks")
    for s in r:
        if s["_id"]["$oid"] == stack_id:
            return s
    return {}


class TestStackDeploy:
    """Basic stack deployment tests."""

    def test_stack_deploy_and_running(self, core, ac_ssh, runner_ssh):
        stack_id = create_stack(core, "test-stack-1", host_port=8095)
        result = deploy_stack(core, stack_id)

        assert result["success"], f"Stack deploy failed: {result.get('logs')}"

        # Verify container running on one of the servers
        ac_names = [c["name"] for c in ac_ssh.podman_ps()]
        runner_names = [c["name"] for c in runner_ssh.podman_ps()]
        assert any("test-stack-1" in n for n in ac_names + runner_names), \
            f"Container not found: AC={ac_names}, Runner={runner_names}"


class TestStackMigration:
    """Test drain-driven stack migration end-to-end."""

    def test_stack_drain_migration(self, core, mongo, ac_ssh, runner_ssh, server_ids):
        """Drain AC; verify stack migrates to runner with source cleanup."""
        stack_id = create_stack(core, "test-stack-migration", host_port=8096, server_id=server_ids["ac"])
        r = deploy_stack(core, stack_id)
        assert r["success"], f"Stack deploy failed: {r.get('logs')}"

        # Verify it's on AC initially
        time.sleep(2)
        ac_names = [c["name"] for c in ac_ssh.podman_ps()]
        assert any("test-stack-migration" in n for n in ac_names), \
            f"Stack not on AC before drain: {ac_names}"

        # Drain AC
        drain_server(core, server_ids["ac"])

        # Wait for migration to complete and AC to be Drained
        drained = wait_for_server_state(core, server_ids["ac"], "Drained", timeout=300)
        assert drained, "AC did not reach Drained state"

        # Verify stack is now on runner
        runner_names = [c["name"] for c in runner_ssh.podman_ps()]
        assert any("test-stack-migration" in n for n in runner_names), \
            f"Stack not on runner after drain: {runner_names}"

        # Verify source container on AC is cleaned up (Polish 1 fix)
        time.sleep(5)
        ac_names = [c["name"] for c in ac_ssh.podman_ps()]
        assert not any("test-stack-migration" in n for n in ac_names), \
            f"Source container still on AC after migration: {ac_names}"

        # Verify target is serving HTTP 200
        status = runner_ssh.curl_status(8096)
        assert status == 200, f"Target not serving HTTP 200: got {status}"

    def test_deployment_drain_migration(self, core, mongo, ac_ssh, runner_ssh, server_ids):
        """Drain AC; verify deployment migrates to runner with source cleanup."""
        dep_cfg = {
            "server_id": server_ids["ac"],
            "image": {"type": "Image", "params": {"image": "nginx:alpine"}},
            "network": "bridge",
            "ports": [{"container": 80, "host": 8097}],
        }
        r = core.write("/CreateDeployment", {"name": "test-dep-migration", "config": dep_cfg})
        dep_id = r["_id"]["$oid"]

        r = core.execute("/Deploy", {"deployment": dep_id})
        update_id = r["_id"]["$oid"]
        result = core.wait_for_update(update_id, timeout=120)
        assert result["success"], f"Deploy failed: {result.get('logs')}"

        # Verify it's on AC
        time.sleep(2)
        ac_names = [c["name"] for c in ac_ssh.podman_ps()]
        assert "test-dep-migration" in ac_names, f"Deployment not on AC: {ac_names}"

        # Drain AC
        drain_server(core, server_ids["ac"])

        # Wait for migration
        drained = wait_for_server_state(core, server_ids["ac"], "Drained", timeout=300)
        assert drained, "AC did not reach Drained state"

        # Verify on runner
        runner_names = [c["name"] for c in runner_ssh.podman_ps()]
        assert "test-dep-migration" in runner_names, f"Deployment not on runner: {runner_names}"

        # Verify source removed from AC
        time.sleep(5)
        ac_names = [c["name"] for c in ac_ssh.podman_ps()]
        assert "test-dep-migration" not in ac_names, \
            f"Source container still on AC: {ac_names}"

        # Verify HTTP 200 on target
        status = runner_ssh.curl_status(8097)
        assert status == 200, f"Target not serving HTTP 200: got {status}"
