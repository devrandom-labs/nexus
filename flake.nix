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
        # Pinned stable toolchain, read from rust-toolchain.toml so rustup
        # users and the flake share one source of truth. No nightly — the
        # crates are stable-clean (issue #204). The sha256 hashes the global
        # channel manifest (platform-independent), so one value covers every
        # system; per-component binaries are fetched per-platform from it.
        rustToolchain = fenix.packages.${system}.fromToolchainFile {
          file = ./rust-toolchain.toml;
          sha256 = "sha256-gh/xTkxKHL4eiRXzWv8KP7vfjSk61Iq48x47BEDFgfk=";
        };
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

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

        # nexus-postgres's DB-backed tests are built once as a cargo-nextest
        # archive, then executed *inside* the NixOS VM below against a live
        # PostgreSQL — so the VM needs no Rust toolchain, only the archive plus
        # `cargo-nextest`. The tests skip (pass) when DATABASE_URL is unset, so
        # they are inert under the normal `nix flake check`; this archive + the
        # Linux-only `postgres-integration` package are what actually run them.
        postgresTests = craneLib.mkCargoDerivation (commonArgs // {
          inherit cargoArtifacts;
          pname = "nexus-postgres-tests";
          doInstallCargoArtifacts = false;
          buildPhaseCargoCommand = ''
            mkdir -p $out
            cargo nextest archive --package nexus-postgres \
              --archive-file $out/nexus-postgres.tar.zst
          '';
          nativeBuildInputs = (commonArgs.nativeBuildInputs or [ ])
            ++ [ pkgs.cargo-nextest ];
        });
      in with pkgs; {
        checks = {
          nexus-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            # Whole-workspace clippy. nexus-framework is temporarily excluded
            # until its projection-runner refactor clears its lint backlog.
            cargoClippyExtraArgs = "--workspace --all-features --lib --exclude nexus-framework -- --deny warnings";
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
          # Run tests with cargo-nextest, fused with cargo-llvm-cov for coverage.
          # withLlvmCov collapses the previous separate tarpaulin step into one
          # instrumented test run — LLVM source-based coverage, no ptrace.
          nexus-nextest = craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;
            withLlvmCov = true;
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
        };

        packages = {
        } // lib.optionalAttrs isLinux {
          # NixOS integration tests require Linux VMs — Linux-only `packages`
          # attribute, deliberately NOT a `checks` entry, so the darwin
          # `nix flake check` dev gate never builds a test archive or boots a VM.
          # Boots a NixOS VM with services.postgresql (a `nexus_test` DB, local
          # trust auth), then runs the pre-built nextest archive against it with
          # DATABASE_URL pointing at the VM's unix-socket Postgres — so the
          # nexus-postgres tests that skip without DATABASE_URL actually execute.
          # Run on Linux/CI with: nix build .#postgres-integration
          postgres-integration = pkgs.testers.runNixOSTest {
            name = "nexus-postgres-integration";
            nodes.machine = { pkgs, ... }: {
              services.postgresql = {
                enable = true;
                ensureDatabases = [ "nexus_test" ];
                # `local all all trust` lets the test process connect over the
                # unix socket as any role without a password — the simplest auth
                # for a throwaway CI VM (no networked Postgres, no TLS).
                authentication = lib.mkForce ''
                  local all all trust
                '';
              };
              environment.systemPackages = [ pkgs.cargo-nextest pkgs.zstd ];
            };
            testScript = ''
              machine.wait_for_unit("postgresql.service")
              # The unix-socket DATABASE_URL form needs a role matching the OS
              # user running the tests. The test script runs as root, so create a
              # `root` superuser role; `nexus_test` is owned by `postgres` and
              # granted to root so the tests can create/truncate the events table.
              machine.succeed("su postgres -c \"psql -c \\\"CREATE ROLE root LOGIN SUPERUSER;\\\"\"")
              machine.copy_from_host(
                  "${postgresTests}/nexus-postgres.tar.zst", "/tmp/tests.tar.zst"
              )
              machine.succeed(
                  "DATABASE_URL='postgres:///nexus_test?host=/run/postgresql' "
                  "cargo nextest run --archive-file /tmp/tests.tar.zst 2>&1 | tee /tmp/out"
              )
            '';
          };
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
            gh
          ];
        };
      });
}
