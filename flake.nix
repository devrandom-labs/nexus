{
  description = "Tixlys Core Microservices";
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    nixpkgs-stable.url = "github:nixos/nixpkgs/nixos-24.11";
    utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    fenix = {
      url = "github:nix-community/fenix";
      inputs = {
        nixpkgs.follows = "nixpkgs";
        rust-analyzer-src.follows = "";
      };
    };
    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
    sops-nix = {
      url = "github:Mic92/sops-nix";
      inputs.nixpkgs.follows = "nixpkgs-stable";
    };
  };
  outputs = { self, nixpkgs, nixpkgs-stable, utils, crane, fenix, advisory-db
    , sops-nix, ... }@inputs:
    utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        inherit (pkgs) lib;
        craneLib = crane.mkLib pkgs;

        unfilteredSrc = ./.;

        src = lib.fileset.toSource {
          root = unfilteredSrc;
          fileset = lib.fileset.unions [
            (craneLib.fileset.commonCargoSources unfilteredSrc)
            ./schemas
          ];
        };

        commonArgs = {
          inherit src;
          strictDeps = true;
          buildInputs = with pkgs;
            [ openssl ] ++ lib.optionals pkgs.stdenv.isDarwin [
              pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
              pkgs.darwin.apple_sdk.frameworks.Security
              pkgs.libiconv
            ];
          nativeBuildInputs = with pkgs;
            [ cmake pkg-config ] ++ lib.optionals pkgs.stdenv.isDarwin [
              pkgs.darwin.apple_sdk.frameworks.Security
              pkgs.darwin.Libsystem
            ];
        };

        craneLibLLvmTools = craneLib.overrideToolchain
          (fenix.packages.${system}.default.toolchain);

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        individualCrateArgs = commonArgs // {
          inherit cargoArtifacts;
          inherit (craneLib.crateNameFromCargoToml { inherit src; }) version;

          doCheck = false;
        };

        mkPackage = name:
          let
            cargoTomlPath = ./bins/${name}/Cargo.toml;
            _c = assert builtins.pathExists cargoTomlPath;
              throw "Cargo file does not exist: ${cargoTomlPath}";
            cargoToml = builtins.fromTOML (builtins.readFile cargoTomlPath);
            pname = cargoToml.package.name;
            version = cargoToml.package.version;
            bin = craneLib.buildPackage (individualCrateArgs // {
              inherit pname version;
              cargoExtraArgs = "-p ${pname}";
            });

            image = pkgs.dockerTools.streamLayeredImage {
              name = "tixlys-core/${pname}";
              created = "now";
              tag = version;
              contents = [ bin ];
              config = {
                Env = [ "RUST_LOG=info,tower_http=trace" ];
                Cmd = [ "${bin}/bin/${pname}" ];
                ExposedPorts = { "3000/tcp" = { }; };
                WorkingDir = "/";
              };
            };
          in image;

        ## crates
        ## personal scripts
        pu = pkgs.writeShellScriptBin "start-infra" ''
          set -euo pipefail
          podman compose up -d
        '';

        pd = pkgs.writeShellScriptBin "stop-infra" ''
          exec podman-compose down
        '';

        i = pkgs.writeShellScriptBin "dive-image" ''
          gunzip --stdout result > /tmp/image.tar && dive docker-archive: ///tmp/image.tar
        '';

        auth = mkPackage "auth";

        ### deploying apps

        mkApp = name:
          let
            imageStream = mkPackage name;
            pushScriptDrv = pkgs.writeShellScriptBin "push-${name}-image" ''
              #!${pkgs.bash}/bin/bash
              set -euo pipefail

              PUSH_LATEST_TAG="''${PUSH_LATEST_TAG:-false}"
              SKOPEO_CMD="${pkgs.skopeo}/bin/skopeo"
              GZIP_CMD="${pkgs.gzip}/bin/gzip"
              IMAGE_STREAM_SCRIPT="${imageStream}"

              DESTINATION="docker://''${REGISTRY_URL,,}/''${IMAGE_NAME,,}:''${IMAGE_TAG}"
              DESTINATION_LATEST="docker://''${REGISTRY_URL,,}/''${IMAGE_NAME,,}:latest"
              CREDENTIALS="''${REGISTRY_USER}:''${REGISTRY_PASSWORD}"

              echo "--- Pushing ${name} Service Image ---"
              echo "Executing stream script: $IMAGE_STREAM_SCRIPT"
              echo "Piping stream via gzip to Skopeo..."
              echo "Source: docker-archive:/dev/stdin"
              echo "Destination: $DESTINATION"
              echo "User: $REGISTRY_USER"

              "$IMAGE_STREAM_SCRIPT" | "$GZIP_CMD" --fast | "$SKOPEO_CMD" copy \
                --dest-creds "$CREDENTIALS" \
                docker-archive:/dev/stdin \
                "$DESTINATION"

              if [[ "$PUSH_LATEST_TAG" == "true" ]]; then
                echo "Pushing latest tag to $DESTINATION_LATEST"
                "$IMAGE_STREAM_SCRIPT" | "$GZIP_CMD" --fast | "$SKOPEO_CMD" copy \
                  --dest-creds "$CREDENTIALS" \
                  docker-archive:/dev/stdin \
                  "$DESTINATION_LATEST"
              fi

              echo "--- Push complete for ${name} ---"
            '';
          in {
            type = "app";
            program = "${pushScriptDrv}/bin/push-${name}-image";
            meta = {
              description =
                "Pushes the ${name} OCI image stream to a configured registry using Skopeo";
            };
          };

      in with pkgs; {

        checks = {
          inherit auth;

          tixlys-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          });

          tixlys-doc =
            craneLib.cargoDoc (commonArgs // { inherit cargoArtifacts; });

          tixlys-fmt = craneLib.cargoFmt { inherit src; };

          tixlys-toml-fmt = craneLib.taploFmt {
            src = pkgs.lib.sources.sourceFilesBySuffices src [ ".toml" ];
            # taplo arguments can be further customized below as needed
            # taploExtraArgs = "format";
          };

          tixlys-audit = craneLib.cargoAudit { inherit src advisory-db; };
          tixlys-deny = craneLib.cargoDeny { inherit src; };
          # Run tests with cargo-nextest
          # Consider setting `doCheck = false` on other crate derivations
          # if you do not want the tests to run twice
          tixlys-nextest = craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;
            partitions = 1;
            partitionType = "count";
          });

          tixlys-coverage =
            craneLib.cargoTarpaulin (commonArgs // { inherit cargoArtifacts; });

          # Ensure that cargo-hakari is up to date
          tixlys-hakari = craneLib.mkCargoDerivation {
            inherit src;
            pname = "tixlys-hakari";
            cargoArtifacts = null;
            doInstallCargoArtifacts = false;

            buildPhaseCargoCommand = ''
              cargo hakari generate --diff  # workspace-hack Cargo.toml is up-to-date
              cargo hakari manage-deps --dry-run  # all workspace crates depend on workspace-hack
              cargo hakari verify
            '';

            nativeBuildInputs = [ cargo-hakari ];
          };

        };

        packages = {
          inherit auth pu pd i;
          ## integration test for auth
          integration = pkgs.testers.runNixOSTest ({
            name = "tixlys-auth-integration-test";
            nodes = { };
            testScript = { nodes, ... }: "\n";
          });
        };

        apps = { auth = mkApp "auth"; };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};

          shellHook = ''
            #!/usr/bin/env bash
            # Create a fancy welcome message
            REPO_NAME=$(basename "$PWD")
            PROPER_REPO_NAME=$(echo "$REPO_NAME" | awk '{print toupper(substr($0,1,1)) tolower(substr($0,2))}')
            figlet "$PROPER_REPO_NAME"
            echo "Welcome to the $PROPER_REPO_NAME development environment on ${system}!"
          '';

          packages = [
            rust-analyzer
            bacon
            figlet
            biscuit-cli
            dive
            cargo-hakari
            tree
            cloc
            skopeo
            gzip
            sops
            age
            cargo-edit
            sqlx-cli
            cargo-expand
          ];
        };
      }) // {
        nixosConfigurations = {
          local = inputs.nixpkgs-stable.lib.nixosSystem {
            system = "x86_64-linux";
            specialArgs = {
              inherit inputs;
              pkgs = inputs.nixpkgs-stable.legacyPackages."x86_64-linux";
            };
            modules = [ ./infra/machines/local.nix ];
          };
        };
      };
}
