# PLACEHOLDER: no target box exists yet. Replace this file with the real
# machine's `nixos-generate-config` output before the first deploy. The
# stub only makes the toplevel buildable for eval checks and the VM.
{ ... }:
{
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;

  fileSystems."/" = {
    device = "/dev/disk/by-label/nixos";
    fsType = "ext4";
  };
}
