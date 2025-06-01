#!/usr/bin/env bash

#
# Runs one of the CI scripts based on the command provided to SSH
#

set -e

cd ~/webring

if [ -z "${SSH_ORIGINAL_COMMAND-x}" ]
then
    echo '$SSH_ORIGINAL_COMMAND is not set' >&2
    exit 1
fi

read -r cmd args <<< "$SSH_ORIGINAL_COMMAND"
case "$cmd" in
    build-for-ci | cleanup | test | deploy )
        exec "./ci/vulcan/${cmd}.sh" $args
        ;;
    *)
        printf 'Invalid command `%s`\n' "$cmd" >&2
        exit 1
        ;;
esac
