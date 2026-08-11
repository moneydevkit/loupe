# Declarative disk layout, consumed by disko when nixos-anywhere installs
# the box. DO droplets expose the root disk as /dev/vda (virtio) and boot
# BIOS (legacy), so GRUB needs a 1M EF02 BIOS-boot partition on the GPT to
# hold core.img; the rest is an ext4 root. For a UEFI box, replace the
# bios partition with a 512M EF00 ESP mounted at /boot and switch the
# loader to systemd-boot in hardware-configuration.nix.
{ ... }:
{
  disko.devices.disk.main = {
    type = "disk";
    device = "/dev/vda";
    content = {
      type = "gpt";
      partitions = {
        bios = {
          size = "1M";
          type = "EF02";
        };
        # Runtime swap headroom for memory-hungry scanner sessions on a
        # small box. disko formats it but does not swapon during install;
        # the installed system activates it at boot via swapDevices.
        swap = {
          size = "8G";
          content = {
            type = "swap";
          };
        };
        root = {
          size = "100%";
          content = {
            type = "filesystem";
            format = "ext4";
            mountpoint = "/";
          };
        };
      };
    };
  };
}
