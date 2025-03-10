{
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
          nativeBuildInputs = with pkgs; [ cmake ];
        };
        craneLibLLvmTools = craneLib.overrideToolchain
          (fenix.packages.${system}.default.toolchain);
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        individualCrateArgs = commonArgs // {
          inherit cargoArtifacts;
          inherit (craneLib.crateNameFromCargoToml { inherit src; }) version;
          doCheck = false;
        };

        fileSetForCrate = crates:
          lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              (craneLib.fileset.commonCargoSources ./crates/errors)
              (craneLib.fileset.commonCargoSources ./crates/cqrs)
              (craneLib.fileset.commonCargoSources crates)
            ];
          };

        mkBinaries = name:
          let
            path = ./bins/${name}/build.nix;
            _ = assert builtins.pathExists path; true;
          in pkgs.callPackage path {
            inherit craneLib fileSetForCrate individualCrateArgs;
          };

        ## crates
        ## personal scripts
        startInfra = pkgs.writeShellScriptBin "start-infra" ''
          set -euo pipefail
          podman compose up -d
        '';

        stopInfra = pkgs.writeShellScriptBin "stop-infra" ''
          exec podman-compose down
        '';

        dive = pkgs.writeShellScriptBin "dive-image" ''
          gunzip --stdout result > /tmp/image.tar && dive docker-archive: ///tmp/image.tar
        '';

      in with pkgs; {
        checks = {
          events = mkBinaries "events";
          auth = mkBinaries "auth";

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
        };

        packages = {
          events = mkBinaries "events";
          auth = mkBinaries "auth";
          start-infra = startInfra;
          stop-infra = stopInfra;
          dive = dive;

          ## FIXME: only put this for darwin? maybe
          tixlys-coverage = craneLibLLvmTools.cargoLlvmCov
            (commonArgs // { inherit cargoArtifacts; });
        };

        devShells.default = let
          pkgsWithUnfree = import nixpkgs {
            inherit system;
            config = { allowUnfree = true; };
          };
        in craneLib.devShell {
          checks = self.checks.${system};
          packages = with pkgsWithUnfree; [
            dive # inspect oci images
            podman
            podman-compose
            kafkactl
            rust-analyzer
            bacon
            sqlx-cli
            biscuit-cli
          ];

          shellHook = ''
            echo "tixlys development environment"
            echo "<<<<<<<<<<<<<<<<<<<< Available Commands >>>>>>>>>>>>>>>>>>>>"
            echo -e "\n\n\n"
            echo "nix build .#events-image [Build events OCI Image]"
            echo "nix build .#events [Build events binary package]"
            echo "nix run .#dive [Run dive on built image]"
            echo -e "\n\n\n"
          '';
        };
      });
}
