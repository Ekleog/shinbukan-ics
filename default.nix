{ release ? false }:

with import ./nix;

naersk.buildPackage {
    pname = "shinbukan-ics";
    version = "dev";

    src = pkgs.lib.sourceFilesBySuffices ./. [".rs" ".toml" ".lock"];

    buildInputs = with pkgs; [
        openssl
        pkg-config
    ];

    inherit release;
}
