{
  description = "Tixlys Core Microservices";
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
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
  };
  outputs = { self, nixpkgs, utils, crane, fenix, advisory-db, ... }:
    utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        inherit (pkgs) lib;
        craneLib = crane.mkLib pkgs;
        src = craneLib.cleanCargoSource ./.;

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

        fileSetForCrate = crate:
          lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              (craneLib.fileset.commonCargoSources ./crates/pawz)
              (craneLib.fileset.commonCargoSources ./crates/nexus)
              (craneLib.fileset.commonCargoSources ./crates/workspace-hack)
              (craneLib.fileset.commonCargoSources crate)
            ];
          };

        ### packaging derivation to build oci image.
        mkPackage = name:
          let
            cratePath = ./bins/${name};
            cargoTomlPath = ./bins/${name}/Cargo.toml;
            _ = assert builtins.pathExists cratePath;
              throw "Path does nopt exist: ${cratePath}";
            _c = assert builtins.pathExists cargoTomlPath;
              throw "Cargo file does not exist: ${cargoTomlPath}";
            cargoToml = builtins.fromTOML (builtins.readFile cargoTomlPath);
            pname = cargoToml.package.name;
            version = cargoToml.package.version;
            bin = craneLib.buildPackage (individualCrateArgs // {
              inherit pname version;
              src = (fileSetForCrate cratePath);
            });

            image = pkgs.dockerTools.streamLayeredImage {
              name = "tixlys-core/${pname}";
              created = "now";
              tag = version;
              contents = [ bin ];
              config = {
                Env = [ "RUST_LOG=info,tower_http=trace" "PORT=3000" ];
                Cmd = [ "${bin}/bin/${pname}" ];
                ExposedPorts = { "3001/tcp" = { }; };
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
          # Audit dependencies
          tixlys-audit = craneLib.cargoAudit { inherit src advisory-db; };

          # # Audit licenses
          tixlys-deny = craneLib.cargoDeny { inherit src; };
          # Run tests with cargo-nextest
          # Consider setting `doCheck = false` on other crate derivations
          # if you do not want the tests to run twice
          tixlys-nextest = craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;
            partitions = 1;
            partitionType = "count";
          });

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

          ## FIXME: only put this for darwin? maybe
          # tixlys-coverage = craneLibLLvmTools.cargoLlvmCov
          #   (commonArgs // { inherit cargoArtifacts; });
        } // lib.optionalAttrs (!pkgs.stdenv.isDarwin) {
          tixlys-coverage = craneLibLLvmTools.cargoLlvmCov
            (commonArgs // { inherit cargoArtifacts; });
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};
          inputsFrom = [ auth ];
          shellHook = ''
            echo "tixlys development environment"
            echo "<<<<<<<<<<<<<<<<<<<< Available Commands >>>>>>>>>>>>>>>>>>>>"
            echo -e "\n\n\n"
            echo "nix build {package-name}"
            echo "nix run .#dive [Run dive on built image]"
            echo -e "\n\n\n"
          '';
          packages = [
            rust-analyzer
            bacon
            biscuit-cli
            dive
            cargo-hakari
            tree
            cloc
            skopeo
          ];
        };
      });
}
