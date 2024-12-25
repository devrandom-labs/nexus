{ pkgs, craneLib, fileSetForCrate, individualCrateArgs }:
let
  cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
  pname = cargoToml.name;
  version = cargoToml.version;
  bin = craneLib.buildPackage (individualCrateArgs // {
    inherit pname;
    cargoExtraArgs = "-p ${pname}";
    src = (fileSetForCrate ./.);
  });
in pkgs.dockerTools.streamLayeredImage {
  name = "tixlys-core/${pname}";
  created = "now";
  tag = version;
  config.Cmd = [ "${bin}/bin/${pname}" ];
}
