let
  pkgs = import ./nix;
in
pkgs.stdenv.mkDerivation {
  name = "shinbukan-ics";
  buildInputs = (
    (with pkgs; [
      cargo-insta
      cargo-nextest
      niv
      openssl
      pkg-config

      (fenix.combine (with fenix; [
        minimal.cargo
        minimal.rustc
        rust-analyzer
      ]))
    ])
  );
}
