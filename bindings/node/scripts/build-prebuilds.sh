#!/bin/bash
# Builds the addon for every platform the npm package supports into prebuilds/<platform>-<arch>/triplelight.node.
# Run on an Apple Silicon Mac: macOS arm64 natively; both Linux addons cross-compiled with cargo-zigbuild, which links
# against glibc 2.31 symbols (the `.2.31` target suffix), so they load on glibc 2.31 or newer. Needs python3 (for
# cargo-zigbuild and ziglang, installed into a throwaway virtualenv) and network access. The script prints the glibc
# symbol version each Linux addon actually needs.
set -euo pipefail
cd "$(dirname "$0")/.."
TOOLCHAIN=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)
GLIBC=2.31
VENV=$(mktemp -d)
python3 -m venv "$VENV"
"$VENV/bin/pip" install -q cargo-zigbuild==0.24.1 ziglang==0.17.1
export PATH="$VENV/bin:$PATH"
rustup toolchain install "$TOOLCHAIN" --profile minimal -c llvm-tools \
  -t aarch64-unknown-linux-gnu -t x86_64-unknown-linux-gnu >/dev/null 2>&1
mkdir -p prebuilds/darwin-arm64 prebuilds/linux-arm64 prebuilds/linux-x64
cargo build --release --locked
cp target/release/libtriplelight_node.dylib prebuilds/darwin-arm64/triplelight.node
cargo zigbuild --release --locked --target "aarch64-unknown-linux-gnu.$GLIBC"
cp target/aarch64-unknown-linux-gnu/release/libtriplelight_node.so prebuilds/linux-arm64/triplelight.node
cargo zigbuild --release --locked --target "x86_64-unknown-linux-gnu.$GLIBC"
cp target/x86_64-unknown-linux-gnu/release/libtriplelight_node.so prebuilds/linux-x64/triplelight.node
OBJDUMP=$(find "$(rustc +"$TOOLCHAIN" --print sysroot)" -name llvm-objdump | head -1)
for t in linux-arm64 linux-x64; do
  printf '%s needs at most ' "$t"
  "$OBJDUMP" -T "prebuilds/$t/triplelight.node" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1
done
rm -rf "$VENV"
ls -l prebuilds/*/triplelight.node
