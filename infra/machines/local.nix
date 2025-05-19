# Edit this configuration file to define what should be installed on
# your system. Help is available in the configuration.nix(5) man page, on
# https://search.nixos.org/options and in the NixOS manual (`nixos-help`).

{ config, lib, pkgs, inputs, ... }:

{
  imports = [
    ../common/base.nix
    # Corrected import using the 'inputs' specialArg
    (inputs.nixpkgs-stable + "/nixos/modules/virtualisation/qemu-vm.nix")
    # # Also include qemu-guest.nix
    # (inputs.nixpkgs-stable + "/nixos/modules/profiles/qemu-guest.nix")
  ];
  # Use the systemd-boot EFI boot loader.

  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;

  networking.useDHCP = true;
  networking.hostName = "tixlys-local";

  virtualisation.qemu.options = [
    "-m 2048" # Allocate 2GB of RAM (up from a small default)
    "-smp 2" # Allocate 2 CPU cores
  ];

  virtualisation.qemu.networkingOptions =
    [ "-net user,hostfwd=tcp::2222-:22" "-net nic" ];

}
