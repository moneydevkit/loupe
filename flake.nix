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

      in
      {
        packages.default = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            doCheck = false; # Tests run in checks.test
          }
        );

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
          ];

          # loupe's bwrap sandbox can't nest the isolate-wrapped agent
          # CLIs, so shadow them with the unsandboxed binaries when the
          # host provides them.
          shellHook = ''
            shims="$PWD/.direnv/agent-shims"
            mkdir -p "$shims"
            rm -f "$shims"/*
            for tool in claude codex; do
              if command -v "$tool-unsandboxed" > /dev/null 2>&1; then
                ln -sf "$(readlink -f "$(command -v "$tool-unsandboxed")")" "$shims/$tool"
              fi
            done
            export PATH="$shims:$PATH"
          '';
        };
      }
    );
}
