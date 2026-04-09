{
  description = "Nexus CQRS, ES, DDD, Hexagonal Arch framework";
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    fenix = {
      url = "github:nix-community/fenix";
      inputs = { nixpkgs.follows = "nixpkgs"; };
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
        isLinux = pkgs.stdenv.isLinux;
        craneLib = (crane.mkLib pkgs).overrideToolchain
          (fenix.packages.${system}.complete.toolchain);

        unfilteredSrc = ./.;

        src = lib.fileset.toSource {
          root = unfilteredSrc;
          fileset = lib.fileset.unions [
            (craneLib.fileset.commonCargoSources unfilteredSrc)
            (lib.fileset.fileFilter (f: f.hasExt "snap") unfilteredSrc)
          ];
        };

        commonArgs = {
          inherit src;
          strictDeps = true;
          buildInputs = with pkgs; [ openssl ];
          nativeBuildInputs = with pkgs; [ cmake pkg-config ];
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      in with pkgs; {
        checks = {
          nexus-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "-p nexus -p nexus-macros --lib -- --deny warnings";
          });

          nexus-doc =
            craneLib.cargoDoc (commonArgs // { inherit cargoArtifacts; });

          nexus-fmt = craneLib.cargoFmt { inherit src; };

          nexus-toml-fmt = craneLib.taploFmt {
            src = pkgs.lib.sources.sourceFilesBySuffices src [ ".toml" ];
            # taplo arguments can be further customized below as needed
            # taploExtraArgs = "format";
          };

          nexus-audit = craneLib.cargoAudit {
            inherit src advisory-db;
            # RUSTSEC-2026-0009: time 0.3.x DoS — transitive dep from refinery 0.8.x
            # Cannot fix until refinery upgrades to rusqlite 0.39+
            cargoAuditExtraArgs = "--ignore RUSTSEC-2026-0009";
          };
          nexus-deny = craneLib.cargoDeny { inherit src; };
          # Run tests with cargo-nextest
          # Consider setting `doCheck = false` on other crate derivations
          # if you do not want the tests to run twice
          nexus-nextest = craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;
            partitions = 1;
            partitionType = "count";
            # Exclude trybuild tests — .stderr snapshots contain absolute paths
            # that differ between local and Nix sandbox environments
            cargoNextestExtraArgs = "-E 'not test(compile_fail)'";
          });

          # Ensure that cargo-hakari is up to date
          nexus-hakari = craneLib.mkCargoDerivation {
            inherit src;
            pname = "nexus-hakari";
            cargoArtifacts = null;
            doInstallCargoArtifacts = false;

            buildPhaseCargoCommand = ''
              cargo hakari generate --diff  # workspace-hack Cargo.toml is up-to-date
              cargo hakari manage-deps --dry-run  # all workspace crates depend on workspace-hack
              cargo hakari verify
            '';
            nativeBuildInputs = [ cargo-hakari ];
          };
        } // lib.optionalAttrs isLinux {
          # cargo-tarpaulin uses ptrace, which is Linux-only
          nexus-coverage =
            craneLib.cargoTarpaulin (commonArgs // {
              inherit cargoArtifacts;
              cargoTarpaulinExtraArgs = "--exclude-files 'tests/compile_fail/*' -- --skip compile_fail";
            });
        };

        packages = {
        } // lib.optionalAttrs isLinux {
          # NixOS integration tests require Linux VMs
          integration = pkgs.testers.runNixOSTest ({
            name = "nexus-integration-test";
            nodes = { };
            testScript = { nodes, ... }: "\n";
          });
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};

          shellHook = ''
            #!/usr/bin/env bash
            # Set git hooks path to tracked .githooks/ directory
            git config core.hooksPath .githooks
            # Create a fancy welcome message
            REPO_NAME=$(basename "$PWD")
            PROPER_REPO_NAME=$(echo "$REPO_NAME" | awk '{print toupper(substr($0,1,1)) tolower(substr($0,2))}')
            figlet -f doom "$PROPER_REPO_NAME" | lolcat -a -d 2
            cowsay -f dragon-and-cow "Welcome to the $PROPER_REPO_NAME development environment on ${system}!" | lolcat
          '';

          packages = [
            fenix.packages.${system}.rust-analyzer
            bacon
            figlet
            lolcat
            cowsay
            tmux
            cargo-hakari
            cargo-mutants
            tree
            cloc
            cargo-edit
            cargo-expand
            gh
          ];
        };
      });
}
