{ config, lib, pkgs }: {

  imports = [ <nixpkgs/nixos/modules/profiles/qemu-guest.nix> ];
  system.stateVersion = "24.11";
  time.timeZone = "Etc/UTC";

  # --- Cloud VM Specifics ---
  boot.loader.grub.enable = true;
  boot.loader.grub.device = "/dev/vda";

  # --- Filesystem Configuration ---
  fileSystems."/" = {
    device = "/dev/vda1";
    fsType = "ext4";
  };

  # --- Networking Configuration ---
  networking.useDHCP = true;
  networking.hostName = "tixlys-local-test";
}
