#!/bin/sh
# Installs the Rust tools BranchBooks builds with, the one place their versions are kept. Used by
# tools/dev/Dockerfile, and by CI for cargo-deny.
#   rust-tools.sh rustup   rustup and the toolchain in rust-toolchain.toml (skip where the base
#                          image has Rust already)
#   rust-tools.sh deny     cargo-deny, which checks advisories, licences and sources (deny.toml)
#   rust-tools.sh ui       dx, the Dioxus CLI that builds crates/branchbooks-ui for Android
# Needs curl and ca-certificates. Linux x86_64 or aarch64.
set -eu

# Keep RUST_VERSION in step with rust-toolchain.toml (CI checks).
RUST_VERSION=1.99.1
CARGO_DENY_VERSION=0.20.2
# Keep DX_VERSION in step with the dioxus version in crates/branchbooks-ui/Cargo.toml (CI checks).
DX_VERSION=0.7.10
RUSTUP_VERSION=1.29.3

# SHA-256 of each download. A new version needs new ones: sha256sum the files, and compare with the
# .sha256 files the project publishes.
arch=$(uname -m)
case "$arch" in
  x86_64)
    RUSTUP_SHA256=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55
    CARGO_DENY_SHA256=9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f
    DX_SHA256=4363e4ed2a3f1eb7f4d38d2d59aed59ce43271c44c16b425e92c89a64761fbe7
    ;;
  aarch64)
    RUSTUP_SHA256=9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282
    CARGO_DENY_SHA256=995c82be0defc7a025cae49a2aa2644ce8245c9a3318fc4103907c6a285e8c7d
    DX_SHA256=8f1a17d3218700ffbe15e6540d936a178b2556fc801121a31082e3ba4ab9ef55
    ;;
  *) echo "rust-tools.sh: unsupported architecture $arch" >&2; exit 1 ;;
esac
bin=${CARGO_HOME:-/usr/local/cargo}/bin

# Downloads $1 to the file $2, and stops unless it has the SHA-256 $3.
fetch() {
  curl --proto '=https' --tlsv1.2 -fsSL "$1" -o "$2"
  echo "$3  $2" | sha256sum -c --quiet -
}

for part in "$@"; do
  case "$part" in
    rustup)
      fetch "https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/$arch-unknown-linux-gnu/rustup-init" \
        /tmp/rustup-init "$RUSTUP_SHA256"
      chmod +x /tmp/rustup-init
      /tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain "$RUST_VERSION" \
        --component clippy,rustfmt
      rm /tmp/rustup-init
      ;;
    deny)
      fetch "https://github.com/EmbarkStudios/cargo-deny/releases/download/$CARGO_DENY_VERSION/cargo-deny-$CARGO_DENY_VERSION-$arch-unknown-linux-musl.tar.gz" \
        /tmp/cargo-deny.tar.gz "$CARGO_DENY_SHA256"
      tar xzf /tmp/cargo-deny.tar.gz -C "$bin" --strip-components=1 "cargo-deny-$CARGO_DENY_VERSION-$arch-unknown-linux-musl/cargo-deny"
      rm /tmp/cargo-deny.tar.gz
      ;;
    ui)
      fetch "https://github.com/DioxusLabs/dioxus/releases/download/v$DX_VERSION/dx-$arch-unknown-linux-gnu.tar.gz" \
        /tmp/dx.tar.gz "$DX_SHA256"
      tar xzf /tmp/dx.tar.gz -C "$bin" dx
      rm /tmp/dx.tar.gz
      ;;
    *) echo "rust-tools.sh: unknown part $part" >&2; exit 1 ;;
  esac
done
