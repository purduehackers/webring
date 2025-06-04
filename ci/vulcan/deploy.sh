#!/usr/bin/env bash

# Deploy a commit as the new webring.

set -e

if [ $# -ne 1 ]; then
  echo Error: expected one argument for the commit hash >&2
  exit 1
fi

HASH=$1

# Fetch and checkout the commit
git fetch origin "$HASH"
git -c advice.detachedHead=false checkout --force "$HASH"

# Reload home manager — Automatically rebuilds the webring and restarts the systemd unit.
home-manager switch
