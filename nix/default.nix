
let
  sources = import ./sources.nix;
in
import sources.nixpkgs {
  overlays = [
    (import (sources.fenix + "/overlay.nix"))
    (self: super: {
      cargo = self.fenix.minimal.cargo;
      rustc = self.fenix.minimal.rustc;
    })
    (import (sources.naersk + "/overlay.nix"))
  ];
}
