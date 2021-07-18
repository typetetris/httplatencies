let
  sources = import nix/sources.nix;
  nixpkgs = import sources.nixpkgs { };
  naersk = nixpkgs.callPackages sources.naersk { };
in
naersk.buildPackage {
  root = ./.;
  nativeBuildInputs = with nixpkgs; [ openssl pkgconfig ];
}
