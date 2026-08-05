# NixOS module for loupe-server and loupe-worker.
#
# Both units expect an already-bootstrapped data directory: run
# `loupe-server init --data-dir <dir> --hostname <name>` once by hand and save
# the admin bundle it prints. Init is deliberately not automated, because it
# prints the admin cert and key exactly once and a unit would bury them in the
# journal.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.loupe;
  inherit (lib)
    mkEnableOption
    mkIf
    mkOption
    optionals
    types
    ;

  serverDir = "/var/lib/${cfg.server.stateDirectory}";
  workerDir = "/var/lib/${cfg.worker.stateDirectory}";
in
{
  options.services.loupe = {
    package = mkOption {
      type = types.package;
      description = "Package providing loupe-server, loupe-worker, and loupectl.";
    };

    server = {
      enable = mkEnableOption "loupe-server";

      bind = mkOption {
        type = types.str;
        default = "127.0.0.1:8443";
        description = "Address:port to listen on for mTLS worker and admin traffic.";
      };

      stateDirectory = mkOption {
        type = types.str;
        default = "loupe-server";
        description = "systemd StateDirectory name, relative to /var/lib.";
      };

      masterKeyFile = mkOption {
        type = types.path;
        description = ''
          File holding the hex-encoded database master key minted by
          `loupe-server init`. Passed through systemd LoadCredential, so it is
          readable only by the unit and never appears in the environment.
        '';
      };

      extraArgs = mkOption {
        type = types.listOf types.str;
        default = [ ];
        description = "Extra arguments for `loupe-server serve`.";
      };
    };

    worker = {
      enable = mkEnableOption "loupe-worker";

      serverUrl = mkOption {
        type = types.str;
        default = "https://127.0.0.1:8443";
        description = "URL of the loupe-server to lease jobs from.";
      };

      stateDirectory = mkOption {
        type = types.str;
        default = "loupe-worker";
        description = "systemd StateDirectory name, relative to /var/lib. Holds the worker cert bundle.";
      };

      agentPackages = mkOption {
        type = types.listOf types.package;
        default = [ ];
        example = lib.literalExpression "[ pkgs.claude-code ]";
        description = ''
          Packages added to the worker's PATH to supply the `claude` and/or
          `codex` CLIs. The worker refuses to start unless at least one is
          present and authenticated, so this is effectively required.
        '';
      };

      environmentFile = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          Root-owned file of `KEY=value` lines holding agent credentials, e.g.
          ANTHROPIC_API_KEY and CODEX_API_KEY. API keys rather than interactive
          logins: the sandbox mounts agent config read-only, so OAuth token
          refresh fails inside it.
        '';
      };

      extraArgs = mkOption {
        type = types.listOf types.str;
        default = [ ];
        example = lib.literalExpression ''[ "--verify-agent" "claude" ]'';
        description = "Extra arguments for `loupe-worker`.";
      };
    };
  };

  config = mkIf (cfg.server.enable || cfg.worker.enable) {
    # Separate users: the worker runs LLM agents over untrusted third-party
    # source, so it must not share an identity with the process holding the
    # database master key.
    users.users = lib.mkMerge [
      (mkIf cfg.server.enable {
        loupe-server = {
          isSystemUser = true;
          group = "loupe-server";
        };
      })
      (mkIf cfg.worker.enable {
        loupe-worker = {
          isSystemUser = true;
          group = "loupe-worker";
        };
      })
    ];
    users.groups = lib.mkMerge [
      (mkIf cfg.server.enable { loupe-server = { }; })
      (mkIf cfg.worker.enable { loupe-worker = { }; })
    ];

    systemd.services.loupe-server = mkIf cfg.server.enable {
      description = "loupe security scanning server";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];

      serviceConfig = {
        ExecStart = lib.escapeShellArgs (
          [
            "${cfg.package}/bin/loupe-server"
            "serve"
            "--bind"
            cfg.server.bind
            "--db"
            "${serverDir}/loupe.sqlite"
            "--server-cert"
            "${serverDir}/server.pem"
            "--server-key"
            "${serverDir}/server.key"
            "--ca-cert"
            "${serverDir}/ca.pem"
            "--ca-key"
            "${serverDir}/ca.key"
          ]
          ++ cfg.server.extraArgs
        );
        User = "loupe-server";
        Group = "loupe-server";
        StateDirectory = cfg.server.stateDirectory;
        StateDirectoryMode = "0700";
        Restart = "on-failure";
        RestartSec = "5s";

        # %d is the credentials directory; the server reads the key from the
        # file rather than the environment.
        LoadCredential = "master-key:${cfg.server.masterKeyFile}";
        Environment = [ "LOUPE_MASTER_KEY_FILE=%d/master-key" ];

        NoNewPrivileges = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
        ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
      };
    };

    systemd.services.loupe-worker = mkIf cfg.worker.enable {
      description = "loupe security scanning worker";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ] ++ optionals cfg.server.enable [ "loupe-server.service" ];
      wants = [ "network-online.target" ];

      # git for repo cloning, bubblewrap for the scanner sandbox, plus whatever
      # agent CLIs the operator supplied.
      path = [
        pkgs.git
        pkgs.bubblewrap
      ]
      ++ cfg.worker.agentPackages;

      serviceConfig = {
        ExecStart = lib.escapeShellArgs (
          [
            "${cfg.package}/bin/loupe-worker"
            "--server-url"
            cfg.worker.serverUrl
            "--ca-cert"
            "${workerDir}/ca.pem"
            "--cert"
            "${workerDir}/worker.pem"
            "--key"
            "${workerDir}/worker.key"
            "--cache-dir"
            "/var/cache/${cfg.worker.stateDirectory}"
          ]
          ++ cfg.worker.extraArgs
        );
        User = "loupe-worker";
        Group = "loupe-worker";
        StateDirectory = cfg.worker.stateDirectory;
        StateDirectoryMode = "0700";
        CacheDirectory = cfg.worker.stateDirectory;
        Restart = "on-failure";
        RestartSec = "10s";
        EnvironmentFile = lib.mkIf (cfg.worker.environmentFile != null) cfg.worker.environmentFile;

        # Hardening is deliberately lighter than the server's. bubblewrap needs
        # to create user and mount namespaces, so RestrictNamespaces,
        # PrivateUsers, and a restrictive SystemCallFilter would all break the
        # scanner sandbox. The sandbox is the isolation boundary here.
        NoNewPrivileges = false;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        # ProtectKernelTunables' /proc overmounts are locked inside bwrap's
        # user namespace, failing the kernel's fully-visible check for the
        # sandbox's fresh /proc mount.
        ProtectKernelTunables = false;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictRealtime = true;
        LockPersonality = true;
        SystemCallArchitectures = "native";
      };
    };
  };
}
