# Deploying loupe on NixOS

This sub-flake turns a fresh box into a NixOS loupe host (server +
worker, kimi scanning and codex verification via OpenRouter).
Everything is built from the parent checkout, so deploying from a
branch needs no package pins or hash bumps. Only git-tracked files
reach the build; `git add -N` new files before deploying.

## First deploy of a new box

Create a KVM box with root SSH; any stock Linux image works, since the
installer replaces it. On DigitalOcean, an Ubuntu 24.04 droplet (4 GB+
RAM) is fine. `disko.nix` and `hardware-configuration.nix` are preset
for a DO virtio guest booting BIOS (legacy) via GRUB; for a UEFI box or
other hardware, adjust them per the notes in those files. From this
directory:

1. Install NixOS onto the box (run from the repo root, where the
   devshell provides `nixos-anywhere`). It kexecs into an in-RAM
   installer, partitions the disk per `disko.nix`, and installs the
   flake's system. `--build-on remote` builds on the box instead of
   uploading a closure over a slow uplink; stock nixpkgs substitutes
   everything from `cache.nixos.org`, so little compiles:

   ```
   nix develop -c nixos-anywhere \
     --flake ./deploy#loupe --build-on remote \
     root@<box>
   ```

   This wipes the disk. The box reboots into NixOS, where the loupe
   units crash-loop: sops can't decrypt yet, because the box's freshly
   generated host key isn't a recipient of `secrets.yaml`.

2. Admit the box to the secrets file. Read its host key:

   ```
   ssh root@<box> cat /etc/ssh/ssh_host_ed25519_key.pub | nix run nixpkgs#ssh-to-age
   ```

   Add the resulting `age1...` key to `.sops.yaml`, then:

   ```
   sops updatekeys secrets.yaml
   ```

3. Set the service's OpenRouter key (a dedicated one, not personal;
   see `.sops.yaml` for editing rules):

   ```
   nix develop -c sops deploy/secrets.yaml    # from the repo root
   ```

4. Deploy, now that the host key and OpenRouter key are in place:

   ```
   nixos-rebuild switch --flake ./deploy#loupe --target-host root@<box>
   ```

5. Bootstrap once, on the box:

   ```
   sudo loupe-bootstrap
   ```

   This mints the CA and certs, registers the worker, and starts both
   units. It stashes the admin bundle in `/root/loupe-admin` — the only
   copy, and there is no admin rotation, so back it up off the box.
   Watch `journalctl -u loupe-worker -f` for
   "bubblewrap available; LLM scanners sandboxed".

6. Register repos (as root, `loupectl` is pre-configured):

   ```
   loupectl repo add --clone-url https://github.com/<owner>/<repo> \
     --no-reporting
   loupectl repo scan 1
   ```

   For a private repo, pass a clone credential (a fine-grained PAT
   with read-only Contents scope on just that repo):

   ```
   LOUPE_CLONE_PAT=github_pat_... loupectl repo add \
     --clone-url https://github.com/<owner>/<repo> --no-reporting
   ```

   For a monorepo, scope the scan to one or more subtrees with
   `--include-path` (repeatable, matched by whole path component).
   Everything outside them is skipped, which keeps the per-file LLM
   fan-out off packages you don't care about:

   ```
   loupectl repo add --clone-url https://github.com/<owner>/<repo> \
     --no-reporting --include-path app/api --include-path app/lib
   ```

   Path scoping is register-time only; to change it, re-register the
   repo (`loupectl repo rm <id>` then add again).

## Day 2

- Redeploy after changes: the same `nixos-rebuild switch` command.
  Bootstrap does not need to run again; it is idempotent if it does.
- Rotate the OpenRouter key: `sops secrets.yaml`, redeploy.
- Rotate a clone PAT: `loupectl repo set-clone-pat <id> --pat ...`
  (or `--clear`).

## Local VM deployment

The same system runs as a QEMU VM on any machine with KVM — identical
config, real secrets, persistent state. Useful both as the actual
deployment (a beefy local box instead of rented hardware) and for
development.

```
nix build ./deploy && ./result/bin/run-loupe-vm
```

The root disk persists as `./loupe.qcow2` in the launch directory, so
`/var/lib/loupe-*` survives restarts like a physical machine; delete
the file to factory-reset. SSH is forwarded on port 2223.

First boot follows the physical-box flow, with the VM's own host key:

```
ssh -p 2223 root@localhost cat /etc/ssh/ssh_host_ed25519_key.pub \
  | nix run nixpkgs#ssh-to-age
# add to .sops.yaml, then: sops updatekeys secrets.yaml
nix build ./deploy && ./result/bin/run-loupe-vm   # rebuild, restart
ssh -p 2223 root@localhost sudo loupe-bootstrap
```

Redeploy after config or loupe changes: rebuild and restart the VM.
The guest mounts the host's /nix/store over 9p, so rebuilds are
incremental. Resources (16G RAM, 8 cores, 64G disk) are set in
`configuration.nix` under `virtualisation.vmVariant`.

Until the host key is admitted, sops decryption fails at activation
and the loupe units crash-loop — the pre-bootstrap state of any fresh
box — which is also all you need for a units-render smoke test.
