#!/bin/sh
# Installs cargo-deny for CI, which checks advisories, licences and sources (deny.toml). Linux x86_64.
# A new version needs a new SHA-256: compare with the .sha256 file the project publishes.
set -eu

VERSION=0.20.2
SHA256=9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f

name=cargo-deny-$VERSION-x86_64-unknown-linux-musl
curl --proto '=https' --tlsv1.2 -fsSL \
  "https://github.com/EmbarkStudios/cargo-deny/releases/download/$VERSION/$name.tar.gz" \
  -o /tmp/cargo-deny.tar.gz
echo "$SHA256  /tmp/cargo-deny.tar.gz" | sha256sum -c --quiet -
mkdir -p "${CARGO_HOME:-$HOME/.cargo}/bin"
tar xzf /tmp/cargo-deny.tar.gz -C "${CARGO_HOME:-$HOME/.cargo}/bin" --strip-components=1 "$name/cargo-deny"
rm /tmp/cargo-deny.tar.gz
