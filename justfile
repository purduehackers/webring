# SPDX-FileCopyrightText: Amolith <amolith@secluded.site>
#
# SPDX-License-Identifier: CC0-1.0

default: reuse lint test staticcheck

reuse:
    reuse lint

lint:
    # Linting Go code
    go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest
    golangci-lint run

test:
    # Running tests
    go test -v ./...

staticcheck:
    # Performing static analysis
    go install honnef.co/go/tools/cmd/staticcheck@latest
    staticcheck ./...

clean:
    # Cleaning up
    rm -rf go-webring
