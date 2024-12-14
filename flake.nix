{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";

    crane.url = "github:ipetkov/crane";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.rust-analyzer-src.follows = "";
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
          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = with pkgs; [ openssl rust-analyzer ];
        };

        craneLibLLvmTools =
          craneLib.overrideToolchain (fenix.packages.${system}.default);
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        individualCrateArgs = commonArgs // {
          inherit cargoArtifacts;
          inherit (craneLib.crateNameFromCargoToml { inherit src; }) version;
          doCheck = false;
        };

        fileSetForCrate = crate:
          lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ];
          };

        coverage = craneLibLLvmTools.cargoLlvmCov
          (commonArgs // { inherit cargoArtifacts; });

      in with pkgs; {

        checks = {

          # Run clippy (and deny all warnings) on the workspace source,
          # again, reusing the dependency artifacts from above.
          #
          # Note that this is done as a separate derivation so that
          # we can block the CI if there are issues here, but not
          # prevent downstream consumers from building our crate by itself.
          stackx-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          });

          stackx-doc =
            craneLib.cargoDoc (commonArgs // { inherit cargoArtifacts; });

          # Check formatting
          stackx-fmt = craneLib.cargoFmt { inherit src; };

          stackx-toml-fmt = craneLib.taploFmt {
            src = pkgs.lib.sources.sourceFilesBySuffices src [ ".toml" ];
            # taplo arguments can be further customized below as needed
            # taploExtraArgs = "format";
          };

          # Audit dependencies
          stackx-audit = craneLib.cargoAudit { inherit src advisory-db; };

          # # Audit licenses
          # stackx-deny = craneLib.cargoDeny { inherit src; };

          # Run tests with cargo-nextest
          # Consider setting `doCheck = false` on other crate derivations
          # if you do not want the tests to run twice
          stackx-nextest = craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;
            partitions = 1;
            partitionType = "count";
          });
        };

        packages = {

        };
        # } // lib.optionalAttrs (!pkgs.stdenv.isDarwin) {
        #   coverage = craneLibLLvmTools.cargoLlvmCov
        #     (commonArgs // { inherit cargoArtifacts; });
        # };
        #

        # TODO: combine all edge services into one service for apps called "edge"
        apps = {

        };
        devShells.default = let
          pkgsWithUnfree = import nixpkgs {
            inherit system;
            config = { allowUnfree = true; };
          };
        in craneLib.devShell {

          checks = self.checks.${system};

          packages = with pkgsWithUnfree; [
            dive
            podman
            podman-compose
            bacon
            ## kafka tool
            kafkactl
          ];
        };
      });
}
