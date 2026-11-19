#!/bin/bash
# Linux (arm64) triplelight binary for the Docker-VM systems (triplelight-vm, triplelight-disk-vm) -> target-linux/release/triplelight.
# Sources are copied into the container first: building straight from the virtiofs bind mount has read stale,
# truncated copies of recently rewritten files (failed build -> suite.sh silently measured the previous binary).
set -eu
cd "$(dirname "$0")/.."
docker run --rm -v $PWD:/src -e CARGO_TARGET_DIR=/src/target-linux rust:latest \
  sh -c "mkdir /b && cp -r /src/src /src/Cargo.toml /src/Cargo.lock /b/ && cd /b && cargo build --release --features server"
