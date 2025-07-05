{ lib, config, ... }: {

  virtualisation.podman = {
    enable = true;
    defaultNetwork.settings.dns_enabled = true;
  };

  sops = {
    defaultSopsFile = ../secrets/prod.yaml;

    # Configure age decryption using the server's own SSH host key(s).
    # sops-nix will attempt to use these private keys (which are generated
    # when NixOS is installed on the VM) to decrypt the secrets file.
    # For th
    # is to work, the corresponding PUBLIC SSH host key(s) of this VM
    # must be listed as a recipient in your main .sops.yaml rule for secrets/prod.yaml.
    age.sshKeyPaths = [ "/etc/ssh/id_ed25519" "/etc/ssh/id_rsa" ];

    # We are using the SSH host keys as age identities, so we don't need
    # a separate file-based age private key for the server itself.
    # Explicitly unsetting these can prevent sops-nix from looking for them if not intended.
    age.keyFile = lib.mkForce null;

    secrets.GHCR_TOKEN = {
      owner = "root";
      mode = "0400";
    };
  };

  virtualisation.oci-containers = {
    backend = "podman";

    registries."ghcr.io".login = {
      username = "tixlys";
      passwordFile = config.sops.secrets.GHCR_TOKEN.path;
    };

    containers.auth = {
      volumes = [ "auth:/config" ];
      image = "ghcr.io/tixlys/tixlys-core/auth:latest";
      ports = [ "8080:3000" ];
      environment = {
        "PORT" = "3000";
        "RUST_LOG" = "info,tixlys=debug";
      };

      healthcheck = {
        test = [ "CMD" "curl" "-f" "http://localhost:3000/health" ];
        interval = "30s";
        timeout = "5s";
        retries = 3;
      };
    };

  };
}

## TODO: add apicurio schema registry
## TODO: add keri witness
## TODO: add liteFS
## TODO: add tixlys receptionist (instead of auth) with its own keri-id
