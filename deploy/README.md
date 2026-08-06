# Deploying loupe on NixOS

This sub-flake turns a box running Determinate NixOS into a loupe host
(server + worker, kimi scanning and codex verification via OpenRouter).
Everything is built from the parent checkout, so deploying from a
branch needs no package pins or hash bumps. Only git-tracked files
reach the build; `git add -N` new files before deploying.

## First deploy of a new box

Install NixOS with the Determinate installer and get root SSH access.
Then, from this directory:

1. Admit the box to the secrets file. On the box:

   ```
   nix run nixpkgs#ssh-to-age < /etc/ssh/ssh_host_ed25519_key.pub
   ```

   Add the resulting `age1...` key to `.sops.yaml`, then:

   ```
   sops updatekeys secrets.yaml
   ```

2. Set the service's OpenRouter key (a dedicated one, not personal;
   see `.sops.yaml` for editing rules):

   ```
   nix develop -c sops deploy/secrets.yaml    # from the repo root
   ```

3. Replace `hardware-configuration.nix` with the box's own
   `nixos-generate-config` output, and put your SSH public key in
   `configuration.nix` (the FIXME).

4. Deploy:

   ```
   nixos-rebuild switch --flake ./deploy#loupe --target-host root@<box>
   ```

   The loupe units crash-loop at this point; they have no certs yet.

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

   For a private repo, pass a clone credential — a fine-grained PAT
   with read-only Contents scope on just that repo:

   ```
   LOUPE_CLONE_PAT=github_pat_... loupectl repo add \
     --clone-url https://github.com/<owner>/<repo> --no-reporting
   ```

## Day 2

- Redeploy after changes: the same `nixos-rebuild switch` command.
  Bootstrap does not need to run again; it is idempotent if it does.
- Rotate the OpenRouter key: `sops secrets.yaml`, redeploy.
- Rotate a clone PAT: `loupectl repo set-clone-pat <id> --pat ...`
  (or `--clear`).

## VM smoke test

```
nix build ./deploy && ./result/bin/run-loupe-vm
```

Builds the same system as a QEMU VM with dummy secrets (nothing in the
VM can decrypt `secrets.yaml`). Good for checking that units render and
the system boots; the units still crash-loop until a bootstrap runs
inside, exactly like a fresh box.
