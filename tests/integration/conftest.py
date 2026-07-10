"""Pytest fixtures for integration tests.

Reads configuration from tests/integration/config.toml.
Provides: core_client, mongo_client, ssh helpers, cleanup fixtures.
"""
import os
import subprocess
import time
import tomllib
from pathlib import Path

import pytest
import requests
from bson import ObjectId
from pymongo import MongoClient

CONFIG_PATH = Path(__file__).parent / "config.toml"


def load_config() -> dict:
    if not CONFIG_PATH.exists():
        pytest.skip(
            f"Integration config not found at {CONFIG_PATH}. "
            "Copy config.example.toml to config.toml."
        )
    with open(CONFIG_PATH, "rb") as f:
        return tomllib.load(f)


CONFIG = load_config()


# ---- HTTP / API ----

class CoreClient:
    """Thin client for the Komodo Core REST API."""

    def __init__(self, url: str, username: str, password: str):
        self._base = url
        self._jwt = None

    def login(self, username: str, password: str):
        r = requests.post(
            f"{self._base}/auth/login/LoginLocalUser",
            json={"username": username, "password": password},
            timeout=10,
        )
        r.raise_for_status()
        self._jwt = r.json()["data"]["jwt"]

    @property
    def jwt(self) -> str:
        if not self._jwt:
            self.login(CONFIG["core"]["username"], CONFIG["core"]["password"])
        return self._jwt

    def _headers(self):
        return {
            "Authorization": f"Bearer {self.jwt}",
            "Content-Type": "application/json",
        }

    def post(self, path: str, body: dict = None) -> dict:
        r = requests.post(
            f"{self._base}{path}",
            json=body or {},
            headers=self._headers(),
            timeout=60,
        )
        r.raise_for_status()
        return r.json()

    def read(self, path: str, body: dict = None) -> dict:
        return self.post(f"/read{path}", body)

    def write(self, path: str, body: dict = None) -> dict:
        return self.post(f"/write{path}", body)

    def execute(self, path: str, body: dict = None) -> dict:
        return self.post(f"/execute{path}", body)

    def wait_for_update(self, update_id: str, timeout: int = 120) -> dict:
        """Poll GetUpdate until status=Complete."""
        for _ in range(timeout // 2):
            r = self.read("/GetUpdate", {"id": update_id})
            if r.get("status") == "Complete":
                return r
            time.sleep(2)
        pytest.fail(f"Update {update_id} did not complete within {timeout}s")

    def list_servers(self) -> list:
        return self.read("/ListFullServers")

    def update_server(self, server_id: str, config: dict) -> dict:
        return self.write("/UpdateServer", {"id": server_id, "config": config})


# ---- SSH ----

class SshRunner:
    """Run commands on a remote host via SSH."""

    def __init__(self, host: str, user: str, key_path: str):
        self.host = host
        self.user = user
        self.key_path = os.path.expanduser(key_path)

    def run(self, cmd: str, timeout: int = 30) -> subprocess.CompletedProcess:
        return subprocess.run(
            [
                "ssh", "-i", self.key_path,
                "-o", "StrictHostKeyChecking=no",
                f"{self.user}@{self.host}",
                cmd,
            ],
            capture_output=True,
            text=True,
            timeout=timeout,
        )

    def podman_ps(self) -> list[dict]:
        """Return list of running containers as {name, status, ports}."""
        r = self.run(
            "podman ps --format '{{.Names}}\t{{.Status}}\t{{.Ports}}'"
        )
        containers = []
        for line in r.stdout.strip().splitlines():
            parts = line.split("\t")
            containers.append({
                "name": parts[0] if len(parts) > 0 else "",
                "status": parts[1] if len(parts) > 1 else "",
                "ports": parts[2] if len(parts) > 2 else "",
            })
        return containers

    def podman_volumes(self) -> list[str]:
        r = self.run("podman volume ls --format '{{.Name}}'")
        return [v.strip() for v in r.stdout.strip().splitlines() if v.strip()]

    def podman_rm(self, name: str):
        self.run(f"podman rm -f {name} 2>/dev/null", timeout=10)

    def curl_status(self, port: int) -> int:
        r = self.run(f"curl -s -o /dev/null -w '%{{http_code}}' http://localhost:{port}/")
        try:
            return int(r.stdout.strip())
        except ValueError:
            return 0


# ---- Fixtures ----

@pytest.fixture(scope="session")
def core() -> CoreClient:
    c = CoreClient(
        CONFIG["core"]["url"],
        CONFIG["core"]["username"],
        CONFIG["core"]["password"],
    )
    c.login(CONFIG["core"]["username"], CONFIG["core"]["password"])
    return c


@pytest.fixture(scope="session")
def mongo():
    cfg = CONFIG["mongo"]
    client = MongoClient(
        f"mongodb://{cfg['username']}:{cfg['password']}@{cfg['host']}:{cfg['port']}/{cfg['db_name']}"
    )
    yield client["komodo"]
    client.close()


@pytest.fixture(scope="session")
def ac_ssh() -> SshRunner:
    s = CONFIG["servers"]["ac"]
    return SshRunner(s["host"], s["user"], CONFIG["ssh"]["key_path"])


@pytest.fixture(scope="session")
def runner_ssh() -> SshRunner:
    s = CONFIG["servers"]["runner"]
    return SshRunner(s["host"], s["user"], CONFIG["ssh"]["key_path"])


@pytest.fixture(scope="session")
def server_ids() -> dict:
    return {
        "ac": CONFIG["servers"]["ac"]["server_id"],
        "runner": CONFIG["servers"]["runner"]["server_id"],
    }


@pytest.fixture(autouse=True, scope="function")
def cleanup_resources(core: CoreClient, mongo, ac_ssh: SshRunner, runner_ssh: SshRunner, server_ids: dict):
    """Clean up all test-* resources before AND after each test."""
    _cleanup_all(core, mongo, ac_ssh, runner_ssh, server_ids)
    yield
    _cleanup_all(core, mongo, ac_ssh, runner_ssh, server_ids)


def _cleanup_all(core: CoreClient, mongo, ac: SshRunner, runner: SshRunner, server_ids: dict):
    """Remove all test deployments/stacks, reset server states, prune containers."""
    # Reset servers to Ok/Run
    for sid in [server_ids["ac"], server_ids["runner"]]:
        mongo.command("update", "Server", updates=[
            {"q": {"_id": ObjectId(sid)},
             "u": {"$set": {"info.state": "Ok", "config.desired_state": "Run", "info.migration_state": None}}}
        ])

    # Delete test deployments and stacks from DB
    for collection in ("Deployment", "Stack"):
        mongo.command("delete", collection, deletes=[{"q": {"name": {"$regex": "^test-"}}, "limit": 0}])

    # Remove test containers and volumes on both servers
    for ssh in (ac, runner):
        # Remove containers with 'test-' in name (but not komodo infrastructure)
        r = ssh.run("podman ps -a --format '{{.Names}}'")
        for name in r.stdout.strip().splitlines():
            if name.startswith("test-"):
                ssh.podman_rm(name)
        # Prune orphaned volumes
        ssh.run("podman volume prune -f", timeout=15)
