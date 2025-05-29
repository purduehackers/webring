#!/usr/bin/env bash

# Clean up the CI environment after having run `test.sh`.

DIR=$(pwd)
cd /
rm -rf $DIR
