# loupe one-time bootstrap. Run as root on the box AFTER the first
# `nixos-rebuild switch` has created the loupe-server / loupe-worker
# users, their StateDirectories, and the sops-decrypted secrets. The
# units will be crash-looping (no certs yet) — that is expected; this
# script populates their state and restarts them.
#
# Idempotent: re-running preserves existing certs and the database.
#
# NOTE: the admin cert+key end up in $ADMIN_DIR and are the ONLY copy —
# loupe has no admin rotation. Back that directory up; losing it means
# you cannot administer the server.
#
# Every path and name below is overridable via environment for
# non-default module settings.

SERVER_DIR=${LOUPE_SERVER_DIR:-/var/lib/loupe-server}
WORKER_DIR=${LOUPE_WORKER_DIR:-/var/lib/loupe-worker}
ADMIN_DIR=${LOUPE_ADMIN_DIR:-/root/loupe-admin}
MASTER_KEY_FILE=${LOUPE_MASTER_KEY_FILE:-/run/secrets/master-key}
SERVER_URL=${LOUPE_SERVER_URL:-https://127.0.0.1:8443}
WORKER_NAME=${LOUPE_WORKER_NAME:-$(hostname)}

[[ $EUID -eq 0 ]] || { echo "must run as root (sudo)"; exit 1; }
[[ -r "$MASTER_KEY_FILE" ]] || {
  echo "master key not readable at $MASTER_KEY_FILE — did sops-nix decrypt it?"
  exit 1
}
log() { printf '\n=== %s ===\n' "$*"; }

log "stopping units during setup"
systemctl stop loupe-worker loupe-server 2>/dev/null || true

# 1. Initialise the server data dir once (mints CA, server cert, admin
#    cert, and the SQLCipher DB). The master key arrives via the env
#    var, so init uses it without persisting a key file of its own.
#    init refuses a populated dir, so guard.
if [[ -f "$SERVER_DIR/ca.key" ]]; then
  log "server data dir already initialised — skipping init"
else
  log "initialising server data dir"
  install -d -m0700 "$SERVER_DIR"
  LOUPE_MASTER_KEY=$(cat "$MASTER_KEY_FILE") \
    loupe-server init --data-dir "$SERVER_DIR" >/dev/null
fi

# 2. Stash the admin bundle for loupectl, then remove it from the data
#    dir (the server never reads it; keeping it there is needless
#    exposure).
if [[ -f "$SERVER_DIR/admin.key" ]]; then
  log "stashing admin bundle to $ADMIN_DIR"
  install -d -m0700 "$ADMIN_DIR"
  install -m0400 "$SERVER_DIR/admin.pem" "$ADMIN_DIR/admin.pem"
  install -m0400 "$SERVER_DIR/admin.key" "$ADMIN_DIR/admin.key"
  install -m0444 "$SERVER_DIR/ca.pem"    "$ADMIN_DIR/ca.pem"
  rm -f "$SERVER_DIR/admin.pem" "$SERVER_DIR/admin.key"
fi

# 3. Hand the data dir to the service user.
chown -R loupe-server:loupe-server "$SERVER_DIR"

# 4. Bring the server up — the worker cert can only be minted while it
#    runs.
log "starting server"
systemctl start loupe-server
export LOUPE_SERVER_URL="$SERVER_URL" \
       LOUPE_CA_CERT="$ADMIN_DIR/ca.pem" \
       LOUPE_ADMIN_CERT="$ADMIN_DIR/admin.pem" \
       LOUPE_ADMIN_KEY="$ADMIN_DIR/admin.key"
ready=
for _ in $(seq 1 30); do
  if loupectl repo list >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
[[ "$ready" == 1 ]] || {
  echo "server did not become ready — check: journalctl -u loupe-server"
  exit 1
}
echo "server responding to loupectl"

# 5. Mint the worker cert bundle once and split it into the worker dir.
if [[ -f "$WORKER_DIR/worker.key" ]]; then
  log "worker cert already present — skipping registration"
else
  log "registering worker"
  install -d -m0700 "$WORKER_DIR"
  # loupectl creates the secret file itself with O_EXCL, so give it a
  # nonexistent path inside a private tempdir (mktemp would pre-create
  # it).
  tmpd=$(mktemp -d)
  loupectl worker register --name "$WORKER_NAME" --out "$tmpd/worker.json" >/dev/null
  jq -r .client_cert_pem "$tmpd/worker.json" > "$WORKER_DIR/worker.pem"
  jq -r .client_key_pem  "$tmpd/worker.json" > "$WORKER_DIR/worker.key"
  jq -r .ca_cert_pem     "$tmpd/worker.json" > "$WORKER_DIR/ca.pem"
  rm -rf "$tmpd"
  chmod 0600 "$WORKER_DIR/worker.key"
fi
chown -R loupe-worker:loupe-worker "$WORKER_DIR"

# 6. Start the worker against the now-populated state.
log "starting worker"
systemctl restart loupe-worker

log "done"
cat <<EOF

Watch the worker come up:
  journalctl -u loupe-worker -f
Expect:
  "bubblewrap available; LLM scanners sandboxed"
  "loupe-worker running"

Administer with loupectl (admin bundle in $ADMIN_DIR):
  export LOUPE_SERVER_URL=$SERVER_URL \\
         LOUPE_CA_CERT=$ADMIN_DIR/ca.pem \\
         LOUPE_ADMIN_CERT=$ADMIN_DIR/admin.pem \\
         LOUPE_ADMIN_KEY=$ADMIN_DIR/admin.key
  loupectl repo list

Register a private repo with a clone credential:
  LOUPE_CLONE_PAT=github_pat_... loupectl repo add \\
    --clone-url https://github.com/<owner>/<repo> --no-reporting
EOF
