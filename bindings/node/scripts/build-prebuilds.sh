#!/bin/bash
# Builds the addon for every platform the npm package supports into prebuilds/<platform>-<arch>/triplelight.node.
# Run on an Apple Silicon Mac with Docker: macOS arm64 natively, Linux arm64 in a container, Linux x64 cross-compiled
# in the same container. Linux addons are built on Debian (glibc), so they need glibc 2.36 or newer to load.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$(cd ../.. && pwd)
TOOLCHAIN=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)
mkdir -p prebuilds/darwin-arm64 prebuilds/linux-arm64 prebuilds/linux-x64
cargo build --release --locked
cp target/release/libtriplelight_node.dylib prebuilds/darwin-arm64/triplelight.node
docker run --rm --platform linux/arm64 -v "$ROOT:/src" -w /src/bindings/node \
  -e CARGO_TARGET_DIR=/tmp/target rust:latest bash -ec "
    apt-get update -qq && apt-get install -y -qq gcc-x86-64-linux-gnu >/dev/null
    rustup toolchain install $TOOLCHAIN --profile minimal -t x86_64-unknown-linux-gnu >/dev/null 2>&1
    cargo +$TOOLCHAIN build --release --locked
    cp /tmp/target/release/libtriplelight_node.so prebuilds/linux-arm64/triplelight.node
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
      cargo +$TOOLCHAIN build --release --locked --target x86_64-unknown-linux-gnu
    cp /tmp/target/x86_64-unknown-linux-gnu/release/libtriplelight_node.so prebuilds/linux-x64/triplelight.node
  "
ls -l prebuilds/*/triplelight.node
