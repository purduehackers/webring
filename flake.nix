# Copyright (C) 2025 Henry Rovnyak
# Copyright (C) 2020 Nicolas Berbiche
#
# This file is part of the Purdue Hackers webring.
#
# The Purdue Hackers webring is free software: you can redistribute it and/or
# modify it under the terms of the GNU Affero General Public License as
# published by the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# The Purdue Hackers webring is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
# or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License
# for more details.
#
# You should have received a copy of the GNU Affero General Public License along
# with the Purdue Hackers webring. If not, see <https://www.gnu.org/licenses/>.

# Edited from...
# https://github.com/berbiche/sample-flake-rust/blob/601a18f165deb771404e8cd177eb0b0ad7c59ad0/flake.nix
# https://github.com/berbiche/sample-flake-rust/blob/601a18f165deb771404e8cd177eb0b0ad7c59ad0/default.nix

{
  description = "The Purdue Hackers Webring";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable"; # We want to use packages from the binary cache
    flake-utils.url = "github:numtide/flake-utils";
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      home-manager,
      rust-overlay,
      ...
    }:
    flake-utils.lib.eachSystem [ "x86_64-linux" ] (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        rust = (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml).override {
          extensions = [
            "rust-src"
            "clippy"
            "rustfmt"
            "llvm-tools-preview"
          ];
        };

        # Use pinned rust version for deployment
        rustPlatform = pkgs.makeRustPlatform {
          rustc = rust;
          cargo = rust;
        };

        cargo = builtins.fromTOML (builtins.readFile ./Cargo.toml);

        infra = with pkgs; [
          prometheus
          grafana
        ];
      in
      rec {
        devShell = pkgs.mkShell {
          buildInputs = [
            rust
          ]
          ++ infra
          ++ (with pkgs; [
            pkg-config
            rust-analyzer
            sccache
            openssl.dev
          ]);

          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
          RUST_BACKTRACE = 1;
          RUSTC_WRAPPER = "sccache";
          DEPLOY = deployBwrap;
        };

        packages.phwebring = rustPlatform.buildRustPackage {
          pname = cargo.package.name;
          version = cargo.package.version;

          src = ./.;

          nativeBuildInputs = [
            pkgs.pkg-config
            pkgs.openssl.dev
          ];
          cargoLock = {
            lockFile = ./Cargo.lock;
            allowBuiltinFetchGit = true;
          };

          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";

          meta = with pkgs.lib; {
            homepage = cargo.package.homepage;
            description = cargo.package.description;
            # TODO: Change
            license = licenses.mit;
          };

          doCheck = false;
        };

        legacyPackages = packages;

        defaultPackage = packages.phwebring;

        deployScript = pkgs.writeShellScript "deploy.sh" ''
            ${pkgs.prometheus}/bin/prometheus --config.file ${./prometheus.yml} &

            if [ ! -d grafana ]; then
              mkdir grafana
            fi

            ln -sfT ${./static} ./static
            ln -sf ${pkgs.grafana}/share/grafana/public grafana
            ln -sf ${pkgs.grafana}/share/grafana/conf grafana
            
            ${pkgs.grafana}/bin/grafana server --homepath grafana --config ${./grafana.ini} &

            ${packages.phwebring}/bin/ph-webring
          '';

        deployBwrap = pkgs.writeShellScript "deployBwrap.sh" ''
            ${pkgs.bubblewrap}/bin/bwrap \
              --bind $DATA_DIR /webring \
              --ro-bind /nix/store /nix/store \
              --ro-bind /etc /etc \
              --tmpfs /tmp \
              --unshare-all \
              --share-net \
              --new-session \
              --chdir /webring \
              --uid 256 \
              --gid 512 \
              --die-with-parent \
              ${deployScript}
          '';

        packages.homeConfigurations."ring" = home-manager.lib.homeManagerConfiguration {
          inherit pkgs;

          modules = [
            {
              home.username = "ring";
              home.homeDirectory = "/home/ring";
              home.stateVersion = "24.11";

              home.packages = [ packages.phwebring ] ++ infra;

              # This will automatically get restarted when rebuilding the home directory
              systemd.user.services.phwebring = {
                Unit.Description = "Purdue Hackers webring";

                Service = {
                  Environment = "DATA_DIR=/home/ring/webring-data";
                  ExecStart = deployBwrap;

                  Restart = "on-failure";
                  Type = "exec";
                };

                Install = {
                  WantedBy = [ "default.target" ];
                };
              };
            }
          ];
        };
      }
    );
}
