# syntax=docker/dockerfile:1.7
#
# AgentGit images. Build from the repository root:
#
#   docker build -f deploy/docker/Dockerfile -t agentgit-server .                 # the server (default)
#   docker build -f deploy/docker/Dockerfile --target cli -t agentgit-cli .       # the `ag` CLI
#
# Both are glibc binaries on a distroless base: no shell, no package manager, a non-root user
# (uid 65532). `.dockerignore` keeps the build context to what the build needs (Cargo.*, crates/, proto/,
# migrations/, and the sources of the Web UI), so editing docs, the SDKs, the UI's tests or the CI files does not
# invalidate any layer.
#
# `ag-server` serves the Web UI from inside its own binary (cargo feature `ui`, crate ag-ui-assets: it embeds
# apps/web/dist at compile time). So the server image is built in this order, and `dist` must exist before the
# final `cargo build`, or the binary compiles without a UI and serves a page that says how to build it.
#
# Layers, in the order they change (least often first), so that CI's buildx cache (type=gha) and a
# developer's local cache both skip the expensive parts:
#   chef      toolchain + cmake/clang + cargo-chef             (changes with RUST_VERSION)
#   planner   recipe.json = the dependency graph of the workspace
#   ui        1. `npm ci`:          the UI's dependencies      (changes with apps/web/package-lock.json)
#             2. scripts/build_ui.sh: apps/web/dist            (changes with apps/web/src)
#   build     1. `cargo chef cook`: compile every dependency   (changes with Cargo.lock / manifests)
#             2. `cargo build`:      compile AgentGit itself   (changes with crates/ or the UI build)
#
# Release builds pass the commit and its date (there is no .git in the build context):
#   docker build ... --build-arg AG_BUILD_COMMIT=$(git rev-parse HEAD) --build-arg AG_BUILD_DATE=$(git show -s --format=%cs HEAD)
# so that `ag-server --version` can say which commit the image was built from.

ARG RUST_VERSION=1.89
ARG CARGO_CHEF_VERSION=0.1.78
# apps/web/package.json asks for Node >= 22.18 (scripts/build_ui.sh checks it too).
ARG NODE_VERSION=22.22

# ---- toolchain ---------------------------------------------------------------------------------
FROM rust:${RUST_VERSION}-bookworm AS chef
ARG CARGO_CHEF_VERSION
# aws-lc-rs and ring compile C/assembly; the `rust` image carries gcc but not always cmake/clang.
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang \
 && rm -rf /var/lib/apt/lists/*
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install cargo-chef --locked --version "${CARGO_CHEF_VERSION}"
WORKDIR /src

# ---- the dependency graph ----------------------------------------------------------------------
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ---- the Web UI ----------------------------------------------------------------------------------
# Only the final `dist` leaves this stage; Node is not part of any image. The dependencies are installed before the
# sources are copied, so editing a component does not reinstall node_modules. `.dockerignore` keeps the host's
# node_modules out of the context, which is why the second COPY cannot overwrite the installed ones.
FROM node:${NODE_VERSION}-bookworm-slim AS ui
WORKDIR /src
COPY apps/web/package.json apps/web/package-lock.json apps/web/
RUN --mount=type=cache,target=/root/.npm \
    cd apps/web && npm ci
COPY apps/web apps/web
COPY scripts/build_ui.sh scripts/build_ui.sh
# AG_UI_SKIP_INSTALL: node_modules is already there. The script checks the Node version, type-checks, builds,
# pre-compresses (.br/.gz next to every compressible file) and fails if dist/index.html is missing.
RUN AG_UI_SKIP_INSTALL=1 bash scripts/build_ui.sh

# ---- compile ---------------------------------------------------------------------------------------
FROM chef AS build
# --locked: the lockfile decides every version. Paths are remapped so the binary does not embed the
# build directory (a step towards reproducible builds). The same flags for cook and build, or the
# cooked dependencies would be rebuilt.
ENV RUSTFLAGS="--remap-path-prefix=/src=/agentgit --remap-path-prefix=/usr/local/cargo=/cargo" \
    CARGO_INCREMENTAL=0
COPY --from=planner /src/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo chef cook --release --locked --recipe-path recipe.json -p agentgit-server -p agentgit-cli
COPY . .
# The UI the server embeds (crates/ag-ui-assets/build.rs looks for ../../apps/web/dist).
COPY --from=ui /src/apps/web/dist apps/web/dist
ARG AG_BUILD_COMMIT=""
ARG AG_BUILD_DATE=""
ENV AG_BUILD_COMMIT=${AG_BUILD_COMMIT} \
    AG_BUILD_DATE=${AG_BUILD_DATE}
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --locked -p agentgit-server -p agentgit-cli \
 && install -D target/release/ag-server /out/ag-server \
 && install -D target/release/ag /out/ag \
 && install -d -m 0700 -o 65532 -g 65532 /out/data

# Git's builtin bundle/pack commands are needed for migration. Copy only the binary and its
# dynamic libraries: no shell, credential helper or network transport is shipped.
FROM debian:bookworm-slim AS git-runtime
RUN apt-get update && apt-get install -y --no-install-recommends git \
 && mkdir -p /git-runtime/usr/bin /git-runtime/licenses/git \
 && cp /usr/bin/git /git-runtime/usr/bin/git \
 && ldd /usr/bin/git | awk '/=> \// { print $3 }' | xargs -I '{}' cp --parents '{}' /git-runtime \
 && cp /usr/share/doc/git/copyright /git-runtime/licenses/git/copyright

# ---- the command-line client ------------------------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot AS cli
LABEL org.opencontainers.image.title="agentgit-cli" \
      org.opencontainers.image.description="The ag command-line client" \
      org.opencontainers.image.source="https://github.com/Paffin/AgentGit" \
      org.opencontainers.image.licenses="Apache-2.0"
COPY --from=build /out/ag /usr/local/bin/ag
COPY LICENSE NOTICE /licenses/
COPY --from=git-runtime /git-runtime/ /
USER 65532:65532
WORKDIR /work
ENTRYPOINT ["/usr/local/bin/ag"]

# ---- the server -----------------------------------------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot AS server
LABEL org.opencontainers.image.title="agentgit-server" \
      org.opencontainers.image.description="The AgentGit server (AGWP over gRPC and HTTP/JSON)" \
      org.opencontainers.image.source="https://github.com/Paffin/AgentGit" \
      org.opencontainers.image.licenses="Apache-2.0"
COPY --from=build /out/ag-server /usr/local/bin/ag-server
COPY --from=build --chown=65532:65532 /out/data /data
COPY deploy/docker/ag-server.toml /etc/agentgit/ag-server.toml
COPY LICENSE NOTICE /licenses/
COPY --from=git-runtime /git-runtime/ /
ENV AG_SERVER_CONFIG=/etc/agentgit/ag-server.toml
USER 65532:65532
VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=15s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/ag-server", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/ag-server"]
CMD ["serve"]
