#!/usr/bin/env bash

#
# Runs one of the CI scripts based on the command provided to SSH
#

if [ -z "${SSH_ORIGINAL_COMMAND-x}" ]
then
    echo '$SSH_ORIGINAL_COMMAND is not set' >&2
    exit 1
fi

case "$SSH_ORIGINAL_COMMAND" in
    build | build-for-ci | cleanup | test )
        exec ./$SSH_ORIGINAL_COMMAND
        ;;
    *)
        printf 'Invalid command `%s`\n' "$SSH_ORIGINAL_COMMAND" >&2
        exit 1
        ;;
esac
