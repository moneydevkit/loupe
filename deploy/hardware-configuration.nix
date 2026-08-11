# Hardware profile for a DO KVM (virtio) guest. disko.nix owns the
# filesystem layout; this file carries only the platform bits a fresh
# nixos-anywhere install needs before the box can regenerate its own
# config. Boot is BIOS (legacy) via GRUB on /dev/vda; for a UEFI box drop
# the GRUB block for `boot.loader.systemd-boot.enable = true` plus
# `boot.loader.efi.canTouchEfiVariables = true`, matching the UEFI note in
# disko.nix.
{ modulesPath, ... }:
{
  imports = [ (modulesPath + "/profiles/qemu-guest.nix") ];

  # disko adds the EF02 disk to grub.devices; we only pick BIOS mode.
  boot.loader.grub = {
    enable = true;
    efiSupport = false;
  };

  boot.initrd.availableKernelModules = [
    "ahci"
    "xhci_pci"
    "virtio_pci"
    "virtio_scsi"
    "virtio_blk"
    "sr_mod"
  ];
}
