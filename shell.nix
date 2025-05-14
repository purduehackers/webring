{ pkgs ? import <nixpkgs> {} }:

/*
based on
https://discourse.nixos.org/t/how-can-i-set-up-my-rust-programming-environment/4501/9
*/
let
  rust_overlay = import (builtins.fetchTarball {
    url = "https://github.com/oxalica/rust-overlay/archive/ceec434b8741c66bb8df5db70d7e629a9d9c598f.tar.gz";
    sha256 = "sha256:0r8vvm8xw17xdqhh2i61ghdqcq0fizi7z64qyck82q7bcy9sksap";
  });
  pkgs = import <nixpkgs> { overlays = [ rust_overlay ]; };
  rust = (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml);
in
pkgs.mkShell {
  buildInputs = [
    rust
  ] ++ (with pkgs; [
    pkg-config
    rust-analyzer
    sccache
    openssl.dev
  ]);

  PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig"; 
  RUST_BACKTRACE = 1;
  RUSTC_WRAPPER = "sccache";
}

