#!/usr/bin/env bash

# Run the webring for either 10s or until it crashes. Returns a non-zero exit code if it crashes. This script must be run with the webring source code as the working directory and after running `build.sh`.

# Sandbox the webring and run it
timeout -k 10s -s 9 10s bwrap --bind /dev/shm/webring-ci /webring --ro-bind /nix/store /nix/store --ro-bind /etc /etc --tmpfs /tmp --unshare-all --share-net --new-session --chdir /webring --uid 256 --gid 512 --die-with-parent ./result/bin/ph-webring
STATUS=$?

if [ $STATUS -ne 137 ]; then
  exit 0
fi
exit $STATUS
