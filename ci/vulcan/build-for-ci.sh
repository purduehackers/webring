#!/usr/bin/env bash

# Build the webring at a particular commit hash and `cd` to the directory where it is located at. Note that you should `source` this script to get the cd to work
# 
# The working directory is expected to be the highest directory of the git repo (~/webring)

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
git fetch --all || exit 1
git --work-tree $CI_PATH checkout $HASH -- . || exit 1
cd $CI_PATH

pwd
./ci/vulcan/build.sh || exit 1
