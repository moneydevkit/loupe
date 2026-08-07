# loupe box: server + worker, agents routed through OpenRouter.
#
# Secrets come from ./secrets.yaml via sops-nix, decrypted on the box by
# its SSH host key. Before the first deploy:
#   1. Add the box to .sops.yaml (ssh-to-age < its ssh_host_ed25519_key.pub)
#      and run `sops updatekeys secrets.yaml`.
#   2. Set the service's dedicated OpenRouter key: see .sops.yaml.
#   3. Replace hardware-configuration.nix with the box's own.
# After the first switch, run `sudo loupe-bootstrap` on the box to mint
# certs and register the worker.
{
  config,
  pkgs,
  loupe,
  ...
}:
let
  loupePkgs = loupe.packages.${pkgs.stdenv.hostPlatform.system};

  # Admin-configured loupectl. The admin bundle is stashed by the bootstrap
  # script, root-owned, so invocations go through sudo.
  loupectl-admin = pkgs.writeShellScriptBin "loupectl" ''
    export LOUPE_SERVER_URL="''${LOUPE_SERVER_URL:-https://127.0.0.1:8443}"
    export LOUPE_CA_CERT="''${LOUPE_CA_CERT:-/root/loupe-admin/ca.pem}"
    export LOUPE_ADMIN_CERT="''${LOUPE_ADMIN_CERT:-/root/loupe-admin/admin.pem}"
    export LOUPE_ADMIN_KEY="''${LOUPE_ADMIN_KEY:-/root/loupe-admin/admin.key}"
    exec ${config.services.loupe.package}/bin/loupectl "$@"
  '';

  # codex 0.133 removed wire_api = "chat"; OpenRouter serves a
  # Responses-compatible /v1/responses, so "responses" is both required and
  # sufficient. env_key must be OPENAI_API_KEY: the worker forwards only
  # that name (plus CODEX_API_KEY) into the sandbox.
  codexHome = pkgs.writeTextDir "config.toml" ''
    model_provider = "openrouter"

    [model_providers.openrouter]
    name = "OpenRouter"
    base_url = "https://openrouter.ai/api/v1"
    env_key = "OPENAI_API_KEY"
    wire_api = "responses"
  '';
in
{
  networking.hostName = "loupe";
  system.stateVersion = "26.05";

  services.openssh.enable = true;
  users.users.root.openssh.authorizedKeys.keys = [
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKj473/+eAlgy1rQwuO+nCRrqhiPAWEgYPIn5j/NdN1Q"
  ];

  sops.defaultSopsFile = ./secrets.yaml;
  sops.age.sshKeyPaths = [ "/etc/ssh/ssh_host_ed25519_key" ];
  sops.secrets.master-key = { };
  sops.secrets.openrouter-api-key = { };
  # Codex reads the OpenRouter key from OPENAI_API_KEY in this env file,
  # forwarded into the sandbox by the worker.
  sops.templates."loupe-agent.env".content = ''
    OPENAI_API_KEY=${config.sops.placeholder.openrouter-api-key}
  '';
  # kimi-code 0.34 dropped env-based provider credentials: an openai-type
  # provider now takes the key inline (`kimi provider list` reports
  # source=inline), with no env indirection. So its config carries the
  # key and must be rendered at runtime rather than into the world-
  # readable store. loupe reads KIMI_CODE_HOME host-side and bind-mounts
  # config.toml to /home/scanner/.kimi-code/config.toml in the sandbox;
  # point it at this template's directory. `--kimi-model` (worker
  # extraArgs) selects the [models.kimi-k3] alias.
  sops.templates."kimi-config.toml" = {
    path = "/run/loupe-kimi/config.toml";
    owner = "loupe-worker";
    content = ''
      default_model = "kimi-k3"
      # Print mode (-p) rejects --yolo/--auto, so headless tool approval is
      # config-driven. "yolo" = approve all; the bwrap sandbox is the boundary.
      default_permission_mode = "yolo"

      [providers.openrouter]
      type = "openai"
      base_url = "https://openrouter.ai/api/v1"
      api_key = "${config.sops.placeholder.openrouter-api-key}"

      [models.kimi-k3]
      provider = "openrouter"
      model = "moonshotai/kimi-k3"
      max_context_size = 262144
    '';
  };

  environment.systemPackages = [
    loupectl-admin
    loupePkgs.bootstrap
    loupePkgs.report
  ];

  services.loupe = {
    server = {
      enable = true;
      masterKeyFile = config.sops.secrets.master-key.path;
    };

    worker = {
      enable = true;
      # loupe's own kimi-code package; codex from nixpkgs. Plain binaries,
      # not isolate-style wrappers, which cannot nest inside loupe's bwrap
      # sandbox.
      agentPackages = [
        loupePkgs.kimi-code
        pkgs.codex
      ];
      environmentFile = config.sops.templates."loupe-agent.env".path;
      # Scan with Kimi K3 on its native harness, verify with Codex on
      # gpt-5.6-terra-pro; both via OpenRouter. Different harness and model
      # family for the second opinion. Verification is low-volume (once per
      # finding, not per file) and correctness-sensitive, so the verifier
      # gets a top reasoning tier.
      extraArgs = [
        "--scan-agent"
        "kimi"
        "--kimi-model"
        "kimi-k3"
        "--verify-agent"
        "codex"
        "--codex-model"
        "openai/gpt-5.6-terra-pro"
      ];
    };
  };

  # Host-side config homes the worker resolves at spawn time to pick its
  # bind-mount sources; the sandbox paths are fixed regardless.
  systemd.services.loupe-worker.environment = {
    KIMI_CODE_HOME = builtins.dirOf config.sops.templates."kimi-config.toml".path;
    CODEX_HOME = "${codexHome}";
  };

  # bkb-mcp on the worker PATH: loupe probes for it and, when present,
  # attaches the Bitcoin Knowledge Base tools to the discovery agent for
  # bitcoin/lightning projects. The store path binds into the sandbox
  # like every other /nix/store dependency. This list merges with the
  # module's own worker PATH (git + bubblewrap + agent CLIs).
  systemd.services.loupe-worker.path = [ loupePkgs.bkb-mcp ];

  # `nix build ./deploy` local VM: the same system, deployable on any
  # machine with KVM. Root disk (./loupe.qcow2) persists across runs, so
  # state survives like a physical box; the host /nix/store is mounted
  # over 9p, so a redeploy is rebuild + restart. Secrets are the real
  # sops path: admit the VM's generated host key exactly like a physical
  # box's (see README), before which units crash-loop, same as any
  # fresh machine.
  virtualisation.vmVariant.virtualisation = {
    memorySize = 16384;
    cores = 8;
    diskSize = 65536;
    diskImage = "./loupe.qcow2";
    graphics = false;
    # Without ballooning, qemu's footprint is a high-water mark: guest
    # pages fault in on demand and nothing ever returns them to the
    # host. Idle vCPUs need no equivalent; KVM halts them.
    qemu.options = [ "-device virtio-balloon,free-page-reporting=on" ];
    # The default writable-store overlay is a tmpfs sized at half of
    # RAM; put store writes on disk instead.
    writableStoreUseTmpfs = false;
    # 2223 because other local VMs already claim 2222.
    forwardPorts = [
      {
        from = "host";
        host.port = 2223;
        guest.port = 22;
      }
    ];
  };
}
