{ pkgs, craneLib, fileSetForCrate, individualCrateArgs }:
let
  cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
  pname = cargoToml.package.name;
  version = cargoToml.package.version;
  bin = craneLib.buildPackage (individualCrateArgs // {
    inherit pname;
    cargoExtraArgs = "-p ${pname}";
    src = (fileSetForCrate ./.);
  });
in pkgs.dockerTools.streamLayeredImage {
  name = "tixlys-core/${pname}";
  created = "now";
  tag = version;
  config = {
    Env = [ "RUST_LOG=info,tower_http=trace" "PORT=3001" ];
    Cmd = [ "${bin}/bin/${pname}" ];
  };
}
