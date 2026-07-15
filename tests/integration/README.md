# Integration tests for luddite deploy

End-to-end tests that exercise the Komodo control plane against real multi-node
infrastructure.

## Prerequisites

1. Two Podman host nodes (referred to as `ac` and `runner`)
2. A running Komodo Core + both Peripheries
3. FerretDB + MinIO accessible (backup tests need S3)
4. Python 3.11+ with `uv` installed

## Setup

```bash
cd tests/integration
cp config.example.toml config.toml
# Edit config.toml with your real SSH keys, IPs, server ObjectIDs

# Create a Python virtual environment and install deps
uv venv
uv pip install -e .
```

You also need an SSH tunnel to FerretDB on the Core host:

```bash
ssh -i ~/.ssh/id_loluet ac@luddite.dev -f -N -L 27018:127.0.0.1:27017
```

## Running

```bash
cd tests/integration
.venv/bin/pytest -v
```

The test suite cleans up all `test-*` resources (containers, volumes, DB rows)
before and after each test, so re-runs are safe.

## What's tested

| Test file           | Coverage                                                                                   |
| ------------------- | ------------------------------------------------------------------------------------------ |
| `test_placement.py` | Adaptive placement: fixed-port routing, port-conflict avoidance, empty server_id scheduler |
| `test_backup.py`    | Volume backup via scheduler, idempotent backups                                            |
| `test_migration.py` | Stack + deployment drain migration, source container cleanup (Polish 1 fix)                |
