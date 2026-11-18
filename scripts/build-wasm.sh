#!/usr/bin/env bash
# Builds the WebAssembly into js/wasm/fast_docx_text.wasm.js (the module as base64, so
# any bundler or runtime carries it as plain JavaScript) and records the sources it
# came from in js/wasm/SOURCES.sha256 (`node scripts/stamp.mjs --check` verifies the
# stamp without Rust).
#
# The bytes depend on the build host (Cargo hashes the host triple into symbol names),
# so x86_64 Linux is the only build host: CI builds natively and compares, and any
# other machine runs the build in the pinned image below (needs Docker). Paths are
# remapped so the checkout location does not matter. `--local` builds with this
# machine's toolchain instead, for iterating; its bytes are not the committed ones.
#
# The module's memory is capped at 1 GiB, and its stack is 8 MiB.
set -euo pipefail
cd "$(dirname "$0")/.."
image="rust:1.96.0@sha256:58fe97504a0e4cbba5d85599619a589923d3e779472a6fb0840d58d1c4ba99d7"
# shellcheck disable=SC2016 # expanded by the build host's shell
build='set -eu
RUSTFLAGS="-C link-arg=--max-memory=1073741824 -C link-arg=-zstack-size=8388608 --remap-path-prefix=$(pwd)=/fast-docx-text --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$(rustc --print sysroot)=/rustc" \
  cargo build --locked --release -p fast-docx-text --lib --target wasm32-unknown-unknown
mkdir -p js/wasm
cp target/wasm32-unknown-unknown/release/fast_docx_text.wasm js/wasm/fast_docx_text.wasm'

if [ "${1:-}" = --local ]; then
  sh -c "$build"
elif [ "$(uname -s)-$(uname -m)" = Linux-x86_64 ]; then
  sh -c "$build"
else
  tar -c --exclude=target Cargo.toml Cargo.lock rust-toolchain.toml crates | docker run --rm -i --platform linux/amd64 "$image" \
    sh -c "mkdir -p /fast-docx-text && cd /fast-docx-text && tar -x && { $build; } >&2 && tar -c js/wasm" |
    tar -x
fi
node scripts/embed-wasm.mjs
rm js/wasm/fast_docx_text.wasm
if [ "${1:-}" != --local ]; then
  node scripts/stamp.mjs --write
fi
