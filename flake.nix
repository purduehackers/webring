# Edited from...
# https://github.com/berbiche/sample-flake-rust/blob/601a18f165deb771404e8cd177eb0b0ad7c59ad0/flake.nix
# https://github.com/berbiche/sample-flake-rust/blob/601a18f165deb771404e8cd177eb0b0ad7c59ad0/default.nix
{
  description = "The Purdue Hackers Webring";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable"; # We want to use packages from the binary cache
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = inputs@{ self, nixpkgs, flake-utils, ... }:
  flake-utils.lib.eachSystem [ "x86_64-linux" ] (system:
    let
      pkgs = nixpkgs.legacyPackages.${system};
      gitignoreSrc = pkgs.callPackage inputs.gitignore { };
      cargo = builtins.fromTOML ( builtins.readFile ./Cargo.toml );
    in rec {
      packages.phwebring = pkgs.rustPlatform.buildRustPackage rec {
        pname = cargo.package.name;
        version = cargo.package.version;

        src = ./.;

        nativeBuildInputs = [ pkgconfig, pkgs.openssl.dev ];
        cargoSha256 = "";

        meta = with stdenv.lib; {
          homepage = cargo.package.homepage;
          description = cargo.package.description;
          # TODO: Change
          license = licenses.mit;
        };
      };

      legacyPackages = packages;

      defaultPackage = packages.phwebring;
    }
  );
}
