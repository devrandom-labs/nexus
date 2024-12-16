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
          nativeBuildInputs = with pkgs; [ ];
          buildInputs = with pkgs; [ ];
        };

        craneLibLLvmTools =
          craneLib.overrideToolchain (fenix.packages.${system}.complete);

        # Build *just* the cargo dependencies (of the entire workspace),
        # so we can reuse all of that work (e.g. via cachix) when running in CI
        # It is *highly* recommended to use something like cargo-hakari to avoid
        # cache misses when building individual top-level-crates
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

      in with pkgs; {

        checks = {
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

        # packages = { } // lib.optionalAttrs (!stdenv.isDarwin) {
        #   tixlys-coverage = craneLibLLvmTools.cargoLlvmCov
        #     (commonArgs // { inherit cargoArtifacts; });
        # };

        devShells.default = let
          pkgsWithUnfree = import nixpkgs {
            inherit system;
            config = { allowUnfree = true; };
          };
        in craneLib.devShell {
          checks = self.checks.${system};
          packages = with pkgsWithUnfree; [ podman podman-compose kafkactl ];
        };

      });
}
