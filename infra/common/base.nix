{ pkgs, ... }: {

  # Set your time zone.
  time.timeZone = "Etc/UTC";

  # This option defines the first version of NixOS you have installed on this particular machine,
  # and is used to maintain compatibility with application data (e.g. databases) created on older NixOS versions.
  #
  # Most users should NEVER change this value after the initial install, for any reason,
  # even if you've upgraded your system to a new NixOS release.
  #
  # This value does NOT affect the Nixpkgs version your packages and OS are pulled from,
  # so changing it will NOT upgrade your system - see https://nixos.org/manual/nixos/stable/#sec-upgrading for how
  # to actually do that.
  #
  # This value being lower than the current NixOS release does NOT mean your system is
  # out of date, out of support, or vulnerable.
  #
  # Do NOT change this value unless you have manually inspected all the changes it would make to your configuration,
  # and migrated your data accordingly.
  #
  # For more information, see `man configuration.nix` or https://nixos.org/manual/nixos/stable/options#opt-system.stateVersion .
  system.stateVersion = "25.05"; # Did you read the comment?
  # Define a user account. Don't forget to set a password with ‘passwd’.
  users.users.tixlys = {
    isNormalUser = true;
    extraGroups = [ "wheel" ];
    initialPassword = "tixlys";
    openssh.authorizedKeys.keys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICuNMvZ7mBPTbASlv7Dg4By/07XW+fs9i+KkYh5xUDWQ joeldsouzax@gmail.com"
    ];
  };

  services.openssh = {
    enable = true;
    settings = {
      PermitRootLogin = "prohibit-password";
      PasswordAuthentication = false;
    };
  };

  networking.firewall = {
    enable = true;
    allowedTCPPorts = [ 22 80 443 ];
  };
  documentation.enable = false;
  documentation.nixos.enable = false;
  documentation.man.enable = false;
  powerManagement.enable = false;
  nix.settings.auto-optimise-store = true;

  # List packages installed in system profile.
  # You can use https://search.nixos.org/ to find more packages (and options).
  environment.systemPackages = with pkgs; [ cowsay lolcat ];

  virtualisation.forwardPorts = [{
    from = "host";
    host.port = 2222;
    guest.port = 22;
  }];
}
