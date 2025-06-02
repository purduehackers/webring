#!/usr/bin/env bash

# Build the webring at a particular commit hash and put it in the `/dev/shm/webring-ci` directory.
#
# The working directory is expected to be the highest directory of the git repo (~/webring)

set -e

if [ $# -ne 1 ]; then
  echo Error: expected one argument for the commit hash >&2
  exit 1
fi

HASH="$1"

# Set up the testing directory in tmpfs

CI_PATH=/dev/shm/webring-ci

# Ensure there isn't a deployment already running
if [ -d "$CI_PATH" ]; then
    echo "Error: $CI_PATH exists. Is there a CI job already running?" >&2
    exit 1
fi

# Prevent other users from deleting/modifying our directory
mkdir "$CI_PATH" --mode=700

# Checkout the commit into the testing directory
# Use `--work-tree` to copy the code into the testing directory instead of the deployment directory. This doesn't copy `.git`.
git fetch origin "$HASH"
git -c advice.detachedHead=false --work-tree "$CI_PATH" restore --source="$HASH" --worktree -- :/
cd "$CI_PATH"

# Build the flake
nix build
