{ ... }: {
  system.stateVersion = "24.05";
  time.timeZone = "Etc/UTC";
  users.users.tixlys = {
    isNormalUser = true;
    description = "Admin user for tixlys servers";
    extraGroups = [ "wheel" "podman" ];

    openssh.authorizedKeys.keys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICuNMvZ7mBPTbASlv7Dg4By/07XW+fs9i+KkYh5xUDWQ joeldsouzax@gmail.com"
    ];
  };

  services.openssh = {
    enable = true;
    settings = {
      PermitRootLogin = "no";
      PasswordAuthentication = false;
    };
  };

  security.sudo.wheelNeedsPassword = true;
  networking.firewall = {
    enable = true;
    allowedTCPPorts = [ 22 80 443 ];
  };

  documentation.enable = false;
  documentation.nixos.enable = false;
  documentation.man.enable = false;
  powerManagement.enable = false;
  nix.settings.auto-optimise-store = true;
}
