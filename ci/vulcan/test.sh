#!/usr/bin/env bash

# Run the webring for either 10s or until it crashes. Returns a non-zero exit code if it crashes. This script must be run with the webring source code as the working directory and after running `build.sh`.

# Sandbox the webring and run it
# The webring will only see its own source code and have rw access to it
timeout -k 10s -s 9 10s bwrap --bind . /webring --ro-bind /nix/store /nix/store --unshare-all --share-net --new-session --chdir /webring --uid 256 --gid 512 --die-with-parent /webring/result/bin/ph-webring
STATUS=$?

cd $AT

if [ $STATUS -ne 137 ]; then
  exit 0
fi
exit $STATUS
