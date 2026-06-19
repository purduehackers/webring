#!/usr/bin/env bash
set -euo pipefail

echo "Updating flake.lock"
ssh vulcan nix flake update --flake "$FLAKE_URL" --output-lock-file "$LOCKFILE"

echo "Trying to build with the new lockfile"
# `set -e` will cause this script to fail if the build fails, acting as a guard clause
ssh vulcan nix build "$FLAKE_URL" --reference-lock-file "$LOCKFILE"

echo "Pushing new lockfile. Deployment will re-run on the new commit."
scp vulcan:"$LOCKFILE" flake.lock

git config user.name "github-actions[bot]"
git config user.email "github-actions[bot]@users.noreply.github.com"

git switch -c $BRANCH_NAME

git add flake.lock
git commit -m "chore: update flake.lock"

git push -u origin $BRANCH_NAME

gh pr create -B master -H $BRANCH_NAME --title "Update flake.lock" --body "Deployment is broken due to an outdated flake"
