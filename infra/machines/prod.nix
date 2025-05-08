{ config, lib, pkgs }: {

  imports =
    [ <nixpkgs/nixos/modules/profiles/qemu-guest.nix> ../common/base.nix ];

  # --- Cloud VM Specifics ---
  boot.loader.grub.enable = true;
  boot.loader.grub.device = "/dev/vda";

  # --- Filesystem Configuration ---
  fileSystems."/" = {
    device = "/dev/vda1";
    fsType = "ext4";
  };

  # Optional: Define swap on a second partition if you create one
  # swapDevices = [ { device = "/dev/vda2"; } ];

  # --- Networking Configuration ---
  networking.useDHCP = true;
  networking.hostName = "tixlys-local-test";
}
