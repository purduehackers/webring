#!/usr/bin/env bash

# Deploy a commit as the new webring.

set -e

if [ $# -ne 1 ]; then
  echo Expected one argument for the commit hash
  exit 1
fi

HASH=$1

# Fetch and checkout the commit
git fetch --all
git checkout $HASH

# Reload home manager — Automatically rebuilds the webring and restarts the systemd unit.
home-manager switch
