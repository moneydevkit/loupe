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
scp := "scp -i " + home_directory() + "/.ssh/id_ed25519 -P " + vm_port + " -F /dev/null -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"

# Deployed droplet host for the `deploy` / `dashboard` recipes. Not hardcoded
# here; the devShell decrypts it from secrets.yaml into LOUPE_DEPLOY_HOST
# (see flake.nix).
droplet_host := env_var_or_default("LOUPE_DEPLOY_HOST", "")

# Host-key flags for nixos-rebuild's SSH to the droplet. Relaxed like the VM
# ssh var; pin the droplet host key before wiring deploys into CI.
nix_sshopts := "-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"

default:
    @just --list

# Reuses any build already realized (e.g. by `deploy-check`). Restarts
# loupe-server/web/worker; a scan in progress is interrupted then re-leased by
# the reaper (re-run, not lost -- see CLAUDE.md). Run `deploy-check` first.
# Build and switch the droplet to the current working tree.
deploy:
    #!/usr/bin/env bash
    set -euo pipefail
    host="{{droplet_host}}"
    [ -n "$host" ] || { echo "no host -- export LOUPE_DEPLOY_HOST" >&2; exit 1; }
    NIX_SSHOPTS="{{nix_sshopts}}" \
        nixos-rebuild switch --flake ./deploy#loupe --target-host "root@$host"

# Builds and copies the closure but does not activate; use before `deploy` to
# see whether a running scan's unit would be restarted.
# Dry-run the deploy and print which units would restart.
deploy-check:
    #!/usr/bin/env bash
    set -euo pipefail
    host="{{droplet_host}}"
    [ -n "$host" ] || { echo "no host -- export LOUPE_DEPLOY_HOST" >&2; exit 1; }
    NIX_SSHOPTS="{{nix_sshopts}}" \
        nixos-rebuild dry-activate --flake ./deploy#loupe --target-host "root@$host"

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

# Render a repo's findings to a self-contained HTML report and copy it
# to ./loupe-findings.html on the host. `just report 2`
report id="1":
    {{ssh}} loupe-report {{id}} -o /tmp/loupe-findings.html
    {{scp}} root@localhost:/tmp/loupe-findings.html ./loupe-findings.html
    @echo "wrote ./loupe-findings.html (open with: xdg-open loupe-findings.html)"

# Forward <host>:8455 (loupe-web's loopback) to localhost and open the current
# token URL in a browser. A fresh token is minted each start and printed to the
# journal; this reads the latest. Ctrl-C closes the tunnel. Shared worker; call
# `dashboard` / `dashboard-local`, not this.
_dashboard host port:
    #!/usr/bin/env bash
    set -euo pipefail
    host="{{host}}"
    if [ -z "$host" ]; then
        echo "no host — export LOUPE_DEPLOY_HOST for the dashboard recipe" >&2
        exit 1
    fi
    opts=(-i {{home_directory()}}/.ssh/id_ed25519 -p {{port}} -F /dev/null
        -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR)
    url=$(ssh "${opts[@]}" "root@$host" 'journalctl -u loupe-web --no-pager --output cat 2>/dev/null | grep -oE "http://127\.0\.0\.1:8455/#t=[A-Za-z0-9_-]+" | tail -1')
    if [ -z "$url" ]; then
        echo "no token URL in the loupe-web journal on $host — is the unit running?" >&2
        exit 1
    fi
    echo "forwarding $host:8455 -> localhost:8455 (Ctrl-C to close)"
    ssh "${opts[@]}" -L 8455:127.0.0.1:8455 -N "root@$host" &
    tunnel=$!
    trap 'kill $tunnel 2>/dev/null || true' EXIT
    sleep 2
    echo "opening $url"
    xdg-open "$url" >/dev/null 2>&1 || echo "open it manually: $url"
    wait $tunnel

# Open the loupe-web operator dashboard on the local VM.
dashboard-local: (_dashboard "localhost" vm_port)

# Open the loupe-web operator dashboard on the deployed droplet (LOUPE_DEPLOY_HOST).
dashboard: (_dashboard droplet_host "22")
