"""Integration tests for volume backup and restore."""
import time

import pytest


def create_deployment_with_volume(core, name, host_port, server_id=""):
    config = {
        "server_id": server_id,
        "image": {"type": "Image", "params": {"image": "nginx:alpine"}},
        "network": "bridge",
        "ports": [{"container": 80, "host": host_port}],
        "volumes": [{"volume": f"{name}-vol", "mount_path": "/usr/share/nginx/html"}],
    }
    r = core.write("/CreateDeployment", {"name": name, "config": config})
    return r["_id"]["$oid"]


def deploy_and_wait(core, deployment_id) -> dict:
    r = core.execute("/Deploy", {"deployment": deployment_id})
    update_id = r["_id"]["$oid"]
    return core.wait_for_update(update_id, timeout=120)


def set_backup_config(core, deployment_id, schedule="* * * * * *"):
    """Set the backup schedule on a deployment via partial config update."""
    core.write("/UpdateDeployment", {
        "id": deployment_id,
        "config": {"backup": {"schedule": schedule, "max_backups": 3}},
    })


def get_deployment_info(core, deployment_id) -> dict:
    """Fetch a single deployment's full representation."""
    deps = core.read("/ListFullDeployments")
    for d in deps:
        if d.get("_id", {}).get("$oid") == deployment_id:
            return d
    return {}


class TestVolumeBackup:
    """Test volume backup lifecycle."""

    def test_volume_backup_via_scheduler(self, core, mongo, ac_ssh, runner_ssh):
        """A deployment with a named volume should get backed up by the scheduler."""
        dep_id = create_deployment_with_volume(core, "test-bk-vol", 8093)
        deploy_and_wait(core, dep_id)
        set_backup_config(core, dep_id)

        # Wait for scheduler to run (60s tick)
        last_backup = {}
        for _ in range(90):
            info = get_deployment_info(core, dep_id)
            last_backup = info.get("info", {}).get("last_backup", {})
            if last_backup:
                break
            time.sleep(2)
        else:
            pytest.fail("Backup did not complete within 180s")

        # last_backup is a HashMap<volume_name, VolumeBackupRecord>
        records = list(last_backup.values())
        assert len(records) > 0
        assert records[0].get("s3_key", "").startswith("backups/")

    def test_idempotent_backup(self, core, mongo, ac_ssh):
        """Running backup multiple times should produce multiple backup records."""
        dep_id = create_deployment_with_volume(core, "test-bk-idem", 8094)
        deploy_and_wait(core, dep_id)
        set_backup_config(core, dep_id)

        # Wait for first backup
        first_key = None
        for _ in range(90):
            info = get_deployment_info(core, dep_id)
            last_backup = info.get("info", {}).get("last_backup", {})
            if last_backup:
                records = list(last_backup.values())
                first_key = records[0].get("s3_key")
                break
            time.sleep(2)

        assert first_key is not None, "First backup did not complete"

        # Wait for second backup (different timestamp → different s3_key)
        found = False
        for _ in range(90):
            info = get_deployment_info(core, dep_id)
            last_backup = info.get("info", {}).get("last_backup", {})
            if last_backup:
                records = list(last_backup.values())
                if records and records[0].get("s3_key") != first_key:
                    found = True
                    break
            time.sleep(2)

        assert found, "Second backup did not produce a different s3_key"
