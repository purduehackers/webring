#!/usr/bin/env bash

# Build the webring at a particular commit hash and put it in the `/dev/shm/webring-ci` directory.
# 
# The working directory is expected to be the highest directory of the git repo (~/webring)

if [ $# -ne 1 ]; then
  echo Expected one argument for the commit hash
  exit 1
fi

HASH=$1

# Set up the testing directory in tmpfs

CI_PATH=/dev/shm/webring-ci

# Create the directory if it's not already created
if ! [ -d $CI_PATH ]; then
  # Prevent other users from deleting/modifying our directory
  mkdir $CI_PATH --mode=700 || exit 1
fi

# Checkout the commit into the testing directory
# Use `--work-tree` to copy the code into the testing directory instead of the deployment directory. This doesn't copy `.git`.
git fetch --all || exit 1
git --work-tree $CI_PATH checkout $HASH -- . || exit 1
cd $CI_PATH || exit 1

# Build the flake
nix build || exit 1
