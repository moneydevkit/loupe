# Deployment flake for a loupe box running Determinate NixOS.
#
# Real box:  nixos-rebuild switch --flake ./deploy#loupe --target-host root@<box>
# VM smoke:  nix build ./deploy && ./result/bin/run-loupe-vm
#
# The loupe input is the parent checkout. Invoked through the git repo
# (./deploy), the fetcher copies tracked files only, so target/ and other
# untracked state never reach the store. Everything the deployment needs
# must therefore be git-tracked.
{
  description = "loupe deployment";

  inputs = {
    loupe.url = "path:..";
    nixpkgs.follows = "loupe/nixpkgs";
    determinate.url = "https://flakehub.com/f/DeterminateSystems/determinate/*";
    sops-nix = {
      url = "github:Mic92/sops-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      loupe,
      determinate,
      sops-nix,
      ...
    }:
    {
      nixosConfigurations.loupe = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        specialArgs = { inherit loupe; };
        modules = [
          loupe.nixosModules.loupe
          determinate.nixosModules.default
          sops-nix.nixosModules.sops
          ./configuration.nix
          ./hardware-configuration.nix
        ];
      };

      # QEMU smoke build of the same system with dummy secrets; see the
      # vmVariant block in configuration.nix.
      packages.x86_64-linux.default = self.nixosConfigurations.loupe.config.system.build.vm;
    };
}
