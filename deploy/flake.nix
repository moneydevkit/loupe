# Deployment flake for a loupe box, built on stock NixOS.
#
# Real box:  nixos-rebuild switch --flake ./deploy#loupe --target-host root@<box>
# VM smoke:  nix build ./deploy && ./result/bin/run-loupe-vm
#
# The loupe input is the parent checkout. Invoked through the git repo
# (./deploy), the fetcher copies tracked files only, so target/ and other
# untracked state never reach the store. Everything the deployment needs
# must therefore be git-tracked.
#
# We deliberately do NOT use Determinate Nix here: its determinate-nix-expr
# (wasmtime) source-builds and OOMs a small box, and its wins (lazy trees,
# the macOS Linux builder, FlakeHub) are developer-workstation features that
# do nothing on a headless scanner. Stock nixpkgs substitutes everything from
# cache.nixos.org, so the install is hands-off on an 8G box.
{
  description = "loupe deployment";

  inputs = {
    loupe.url = "path:..";
    nixpkgs.follows = "loupe/nixpkgs";
    sops-nix = {
      url = "github:Mic92/sops-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    disko = {
      url = "github:nix-community/disko";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      loupe,
      sops-nix,
      disko,
      ...
    }:
    {
      nixosConfigurations.loupe = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        specialArgs = { inherit loupe; };
        modules = [
          loupe.nixosModules.loupe
          sops-nix.nixosModules.sops
          disko.nixosModules.disko
          ./configuration.nix
          ./hardware-configuration.nix
          ./disko.nix
        ];
      };

      # QEMU smoke build of the same system with dummy secrets; see the
      # vmVariant block in configuration.nix.
      packages.x86_64-linux.default = self.nixosConfigurations.loupe.config.system.build.vm;
    };
}
