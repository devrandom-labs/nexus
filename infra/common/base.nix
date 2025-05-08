{ config, lib, pkgs, ... }: {
  system.stateVersion = "24.11";
  time.timeZone = "Etc/UTC";
  users.users.tixlys = {
    isNormalUser = true;
    description = "Admin user for tixlys servers";
    extraGroups = [ "wheel" "podman" ];
  };
}
