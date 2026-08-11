{
  description = "loupe";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      fenix,
      crane,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        fenixPkgs = fenix.packages.${system};

        # Stable toolchain plus nightly rustfmt: rustfmt.toml uses
        # nightly-only options. Matches CI (fmt on nightly, rest on stable).
        toolchain = fenixPkgs.combine [
          (fenixPkgs.stable.withComponents [
            "cargo"
            "clippy"
            "rust-src"
            "rustc"
          ])
          fenixPkgs.latest.rustfmt
        ];

        craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;

        # Run a toolchain binary with the credential dirs masked, so a
        # malicious build.rs / proc-macro from a pulled crate can't read
        # them. Denylist sandbox: the whole fs stays visible except the
        # tmpfs'd secret dirs, so cargo still fetches,
        # builds, and writes target/ normally. Only dirs that exist are
        # masked, since bwrap errors on a missing --tmpfs target. The
        # toolchain goes first on PATH inside the sandbox so a nested cargo
        # resolves the real binary instead of re-entering this wrapper.
        sandboxSecrets = name: real:
          pkgs.writeShellScriptBin name ''
            # The wrapped tool runs untrusted crate build scripts, so drop the
            # secrets it never needs; masking the age-key file is moot if the
            # already-decrypted values sit in the inherited env. CARGO_REGISTRY_TOKEN
            # is the classic supply-chain target and a build never needs it.
            unset LOUPE_CLONE_PAT LOUPE_DEPLOY_HOST CARGO_REGISTRY_TOKEN
            args=()
            for d in "$HOME/.config/sops" "$HOME/.ssh" "$HOME/.gnupg" \
                     "$HOME/.aws" "$HOME/.config/gh" "$HOME/.config/gcloud"; do
              [ -e "$d" ] && args+=(--tmpfs "$d")
            done
            exec ${pkgs.bubblewrap}/bin/bwrap --dev-bind / / \
              --setenv PATH "${toolchain}/bin:$PATH" \
              "''${args[@]}" ${real} "$@"
          '';
        sandboxedCargo = sandboxSecrets "cargo" "${toolchain}/bin/cargo";
        # rust-analyzer runs the same build.rs / proc-macros from the editor,
        # so it needs the same masking. VS Code must be told to use this via
        # rust-analyzer.server.path (see .vscode/settings.json); editors that
        # resolve rust-analyzer from PATH pick up the shim for free.
        sandboxedRustAnalyzer = sandboxSecrets "rust-analyzer" "${pkgs.rust-analyzer}/bin/rust-analyzer";

        # Keep loupe-web's assets (index.html / app.css / app.js) that its
        # `include_str!` needs; cleanCargoSource would otherwise strip them.
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          name = "source";
          filter =
            path: type:
            (builtins.match ".*/crates/loupe-web/assets/.*" path != null)
            || (craneLib.filterCargoSources path type);
        };

        commonArgs = {
          inherit src;
          pname = "loupe";
          version = "0.0.0";
          strictDeps = true;
          # bundled-sqlcipher-vendored-openssl runs perl at build time.
          nativeBuildInputs = [ pkgs.perl ];
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        loupe = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            doCheck = false; # Tests run in checks.test
            meta.mainProgram = "loupe-server";
          }
        );
      in
      {
        # One derivation carries all three binaries (loupe-server,
        # loupe-worker, loupectl); `loupe` is just a named alias for it.
        packages = {
          default = loupe;
          inherit loupe;

          # One-time post-switch setup on a deployed box: mints certs,
          # stashes the admin bundle, registers the worker. The deploy
          # flake installs it as `loupe-bootstrap`.
          bootstrap = pkgs.writeShellApplication {
            name = "loupe-bootstrap";
            runtimeInputs = [
              loupe
              pkgs.jq
            ];
            text = builtins.readFile ./nix/bootstrap.sh;
          };

          # Renders a repo's findings to a self-contained HTML file by
          # shelling loupectl. Stdlib only; runs on the box where the
          # admin bundle lives (see `just report`).
          report = pkgs.writers.writePython3Bin "loupe-report" {
            flakeIgnore = [
              "E501" # long lines: the inline CSS strings are intentional.
              "E226" # missing whitespace around arithmetic operators.
              "W503" # line break before binary operator (PEP8 now prefers it).
            ];
          } (builtins.readFile ./nix/loupe-report.py);
        }
        // {
          # Bitcoin Knowledge Base MCP server for the discovery agent.
          bkb-mcp = pkgs.callPackage ./nix/bkb-mcp.nix { };
        }
        # Binary release, linux-x64 only.
        // pkgs.lib.optionalAttrs (system == "x86_64-linux") {
          kimi-code = pkgs.callPackage ./nix/kimi-code.nix { };
        };

        checks = {
          clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--workspace --all-targets --all-features -- --deny warnings";
            }
          );

          fmt = craneLib.cargoFmt {
            inherit src;
            pname = "loupe";
            version = "0.0.0";
          };

          test = craneLib.cargoTest (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoTestExtraArgs = "--workspace --all-targets";
              # Worker tests shell out to git for repo cloning.
              nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.git ];
            }
          );
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};

          packages = with pkgs; [
            rust-analyzer
            nixfmt
            jq
            # Edits deploy/secrets.yaml; see deploy/.sops.yaml.
            sops
            # Deploy / local-VM operations; see ./justfile.
            just
            # Installs the deploy flake onto a fresh box; see deploy/README.md.
            nixos-anywhere
          ];

          # loupe's bwrap sandbox can't nest the isolate-wrapped agent
          # CLIs, so shadow them with the unsandboxed binaries when the
          # host provides them.
          shellHook = ''
            shims="$PWD/.direnv/agent-shims"
            mkdir -p "$shims"
            rm -f "$shims"/*
            for tool in claude codex kimi; do
              if command -v "$tool-unsandboxed" > /dev/null 2>&1; then
                ln -sf "$(readlink -f "$(command -v "$tool-unsandboxed")")" "$shims/$tool"
              fi
            done
            # Shadow cargo and rust-analyzer with the credential-masking
            # sandbox wrapper (both execute crate build scripts).
            ln -sf ${sandboxedCargo}/bin/cargo "$shims/cargo"
            ln -sf ${sandboxedRustAnalyzer}/bin/rust-analyzer "$shims/rust-analyzer"
            export PATH="$shims:$PATH"

            # Load developer-local secrets (deploy host, clone PAT) from the
            # sops-encrypted secrets.yaml into the env for the just recipes.
            # Best-effort: a missing file or age key is a silent no-op, so the
            # shell still works for contributors without the operator key.
            if [ -f "$PWD/secrets.yaml" ] && command -v sops > /dev/null 2>&1; then
              if host=$(sops -d --extract '["deploy-host"]' "$PWD/secrets.yaml" 2>/dev/null); then
                export LOUPE_DEPLOY_HOST="$host"
              fi
              if pat=$(sops -d --extract '["clone-pat"]' "$PWD/secrets.yaml" 2>/dev/null); then
                export LOUPE_CLONE_PAT="$pat"
              fi
            fi
          '';
        };
      }
    )
    // {
      # NixOS modules are not per-system, so they live outside
      # eachDefaultSystem. ./nix/loupe.nix is a plain module with no flake
      # dependency; this wrapper only defaults the package to our build.
      nixosModules.loupe =
        {
          pkgs,
          lib,
          ...
        }:
        {
          imports = [ ./nix/loupe.nix ];
          services.loupe.package = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.loupe;
        };
      nixosModules.default = self.nixosModules.loupe;
    };
}
