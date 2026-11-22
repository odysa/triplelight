#!/bin/bash
# Builds the addon for every platform the npm package supports into prebuilds/<platform>-<arch>/triplelight.node.
# Run on an Apple Silicon Mac with Docker: macOS arm64 natively, Linux arm64 in a container, Linux x64 cross-compiled
# in the same container. The Linux addons are built on Debian 11 (bullseye, glibc 2.31) with the pinned toolchain, so they
# load on glibc 2.31 or newer; the script prints the glibc symbol version each addon actually needs.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$(cd ../.. && pwd)
TOOLCHAIN=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)
mkdir -p prebuilds/darwin-arm64 prebuilds/linux-arm64 prebuilds/linux-x64
cargo build --release --locked
cp target/release/libtriplelight_node.dylib prebuilds/darwin-arm64/triplelight.node
docker run --rm --platform linux/arm64 -v "$ROOT:/src" -w /src/bindings/node \
  -e CARGO_TARGET_DIR=/tmp/target debian:bullseye bash -ec "
    # Debian 11 is end-of-life: its packages live on archive.debian.org, and its security repository is gone.
    sed -i 's|deb.debian.org|archive.debian.org|; /security/d' /etc/apt/sources.list
    apt-get update -qq
    apt-get install -y -qq build-essential curl ca-certificates gcc-x86-64-linux-gnu binutils-multiarch >/dev/null
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none --profile minimal >/dev/null 2>&1
    . /root/.cargo/env
    rustup toolchain install $TOOLCHAIN --profile minimal -t x86_64-unknown-linux-gnu >/dev/null 2>&1
    cargo +$TOOLCHAIN build --release --locked
    cp /tmp/target/release/libtriplelight_node.so prebuilds/linux-arm64/triplelight.node
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
      cargo +$TOOLCHAIN build --release --locked --target x86_64-unknown-linux-gnu
    cp /tmp/target/x86_64-unknown-linux-gnu/release/libtriplelight_node.so prebuilds/linux-x64/triplelight.node
    for t in linux-arm64 linux-x64; do
      printf '%s needs at most ' \$t
      objdump -T prebuilds/\$t/triplelight.node | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1
    done
  "
ls -l prebuilds/*/triplelight.node
