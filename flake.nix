{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
  };
  outputs = { nixpkgs, utils, ... }:
    utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        ## TODO: need jikkou in development shell for kafka state management
      in with pkgs; {
        devShells.default =
          mkShell { buildInputs = [ podman podman-compose kafkactl ]; };
      });
}
