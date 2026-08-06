# loupe deploy / local-VM operations. See deploy/README.md for the full
# first-boot flow (admitting the VM host key to sops, bootstrap).
#
# The VM is the deploy flake built as a QEMU image; state persists in
# ./loupe.qcow2. loupectl on the VM runs as root pre-configured with the
# admin bundle, so the repo/job recipes just SSH in and call it.
#
# Code validation is NOT here; use the flake checks (see CLAUDE.md):
#   nix develop -c cargo clippy --workspace --all-targets --all-features
#   nix develop -c cargo test --workspace --all-targets

vm_port := "2223"
# -F /dev/null: skip user/system ssh_config, whose nix-store symlinks can
# abort ssh. Host key is unpinned because it is regenerated whenever
# loupe.qcow2 is recreated.
ssh := "ssh -i " + home_directory() + "/.ssh/id_ed25519 -p " + vm_port + " -F /dev/null -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR root@localhost"

default:
    @just --list

# Build the VM image from the current working tree.
vm-build:
    nix build ./deploy

# Rebuild and restart the VM against the persistent disk, wait for SSH.
# State (certs, DB, registered repos) survives, so no re-bootstrap.
vm-redeploy: vm-build vm-stop
    #!/usr/bin/env bash
    set -euo pipefail
    echo "starting VM..."
    nohup ./result/bin/run-loupe-vm >/dev/null 2>&1 &
    for _ in $(seq 1 120); do
        {{ssh}} -o ConnectTimeout=2 true 2>/dev/null && { echo "VM ready"; exit 0; }
        sleep 2
    done
    echo "error: VM did not answer on port {{vm_port}}" >&2; exit 1

# Stop the VM (graceful poweroff; falls back to killing qemu).
vm-stop:
    #!/usr/bin/env bash
    set -euo pipefail
    if {{ssh}} -o ConnectTimeout=2 poweroff 2>/dev/null; then :; fi
    for _ in $(seq 1 30); do
        pgrep -f 'qemu.*run-loupe-vm|qemu.*loupe.qcow2' >/dev/null || { echo "VM stopped"; exit 0; }
        {{ssh}} -o ConnectTimeout=1 true 2>/dev/null || sleep 1
    done
    pkill -f 'qemu.*loupe.qcow2' 2>/dev/null || true
    echo "VM stopped"

# First-boot (or post-reset) setup on the VM: mint certs, register worker.
bootstrap:
    {{ssh}} loupe-bootstrap

# SSH into the VM as root.
vm-ssh:
    {{ssh}}

# Follow the server + worker journals.
vm-logs:
    {{ssh}} journalctl -f -u loupe-server -u loupe-worker --output cat

# Dump the last N worker log lines and exit (non-interactive).
vm-logs-dump lines="200":
    {{ssh}} journalctl --no-pager -n {{lines}} -u loupe-worker --output cat

# loupectl passthrough, e.g. `just ctl repo list` or `just ctl finding show 1`.
ctl *args:
    {{ssh}} loupectl {{args}}

# Register a public repo for scanning. `just repo-add https://github.com/o/r`
repo-add url *args:
    {{ssh}} loupectl repo add --clone-url {{url}} --no-reporting {{args}}

# Register a private repo, reading the clone PAT from $LOUPE_CLONE_PAT.
# `LOUPE_CLONE_PAT=github_pat_... just repo-add-private https://github.com/o/r`
repo-add-private url *args:
    {{ssh}} LOUPE_CLONE_PAT="${LOUPE_CLONE_PAT:?set LOUPE_CLONE_PAT}" \
        loupectl repo add --clone-url {{url}} --no-reporting {{args}}

# Trigger a scan now. `just scan 1`
scan id:
    {{ssh}} loupectl repo scan {{id}}

# List registered repos.
repos:
    {{ssh}} loupectl repo list

# List jobs.
jobs:
    {{ssh}} loupectl job list

# List findings for a repo. `just findings 1`
findings id:
    {{ssh}} loupectl finding list {{id}}
