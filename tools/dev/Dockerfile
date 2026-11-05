# BranchBooks development image: every tool the repository needs, so nothing is installed on the
# host. Rust builds and tests everything (tools/dev/rust-tools.sh has the versions); git checks the
# git store's repositories in tests; real Beancount (the oracle, never linked to BranchBooks) sits
# in its own virtualenv. No Node: the repository has no npm.
# Used by compose.yaml in the repository root (see README "Setup"). CI runs without it, and only checks
# that it builds (.github/workflows/dev-image.yml).

# By digest, so the image is what was tested and Dependabot can propose each new one.
FROM debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4

# The image's dev user takes these ids, so what it writes to the bind-mounted repository is yours.
# compose.yaml passes UID and GID from the environment, defaulting to 1000.
ARG UID=1000
ARG GID=1000
COPY oracle-requirements.txt /tmp/oracle-requirements.txt
# gcc and libc6-dev: the linker cargo needs for native builds (tests, clippy).
RUN apt-get update && apt-get install -y --no-install-recommends git ca-certificates curl gcc libc6-dev \
    python3 python3-venv \
  && rm -rf /var/lib/apt/lists/* \
  && python3 -m venv /opt/beancount \
  && /opt/beancount/bin/pip install --no-cache-dir -r /tmp/oracle-requirements.txt \
  && rm /tmp/oracle-requirements.txt \
  && git config --system --add safe.directory '*' \
  && groupadd -o -g "$GID" dev \
  && useradd -o -m -u "$UID" -g "$GID" -s /bin/bash dev

# Installed as root, then cargo's home is handed to dev so the registry volume and cargo install
# work in the container.
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
COPY --chmod=755 rust-tools.sh /usr/local/bin/rust-tools.sh
RUN rust-tools.sh rustup deny ui \
  && mkdir -p /usr/local/cargo/registry /src/target \
  && chown -R dev:dev /usr/local/cargo /src

# The repository is bind-mounted at /src. cargo's registry and target directory live in named
# volumes, which take dev's ownership from the directories made above.
ENV BEANCOUNT_PYTHON=/opt/beancount/bin/python
USER dev
WORKDIR /src
CMD ["bash"]
