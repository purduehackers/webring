#!/usr/bin/env bash

if [ $# -ne 1 ]; then
  echo Expected one argument for the commit hash
  exit 1
fi

HASH=$1

# Set up the testing directory in tmpfs

CI_PATH=/dev/shm/"$HASH"-webring-ci

# Prevent other users from deleting/modifying our directory
mkdir $CI_PATH --mode=700

# Defer cleanup until after this script has finished
# SCRIPT_PID=$$
# bash -e "wait $SCRIPT_PID; rm -r $CI_PATH" &
# disown

# Checkout the commit into the testing directory
# Use `--work-tree` to copy the code into the testing directory instead of the deployment directory. This doesn't copy `.git`.
mkdir $CI_PATH/webring
git fetch --all || exit 1
git --work-tree $CI_PATH/webring checkout $HASH || exit 1

# Build the flake
AT=$(pwd)
cd $CI_PATH
nix build || exit 1

# Sandbox the webring and run it
# The webring will only see its own source code and have rw access to it
timeout -k 10s -s 9 bwrap -- \
  --bind $CI_PATH / --unshare-user --uid 256 --gid 512 ./result/bin/ph-webring
STATUS=$?

cd $AT

if [ $STATUS -ne 137 ]; then
  exit 0
fi
exit $STATUS
