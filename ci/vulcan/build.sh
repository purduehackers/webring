#!/usr/bin/env bash

# Build the flake
nix build || exit 1

cp ./result/bin/ph-webring .
rm result
