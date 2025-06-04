#!/usr/bin/env bash

# Clean up the CI environment after having run `test.sh`.

set -e

cd /
rm -rf /dev/shm/webring-ci
