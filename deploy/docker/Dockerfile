# syntax=docker/dockerfile:1.7
#
# AgentGit images. Build from the repository root:
#
#   docker build -f deploy/docker/Dockerfile -t agentgit-server .                 # the server (default)
#   docker build -f deploy/docker/Dockerfile --target cli -t agentgit-cli .       # the `ag` CLI
#
# Both are glibc binaries on a distroless base: no shell, no package manager, a non-root user
# (uid 65532). `.dockerignore` keeps the build context to what cargo needs (Cargo.*, crates/, proto/,
# migrations/), so editing docs, the web UI or the SDKs does not invalidate any layer.
#
# Layers, in the order they change (least often first), so that CI's buildx cache (type=gha) and a
# developer's local cache both skip the expensive parts:
#   chef      toolchain + cmake/clang + cargo-chef             (changes with RUST_VERSION)
#   planner   recipe.json = the dependency graph of the workspace
#   build     1. `cargo chef cook`: compile every dependency   (changes with Cargo.lock / manifests)
#             2. `cargo build`:      compile AgentGit itself   (changes with crates/)
#
# Release builds pass the commit and its date (there is no .git in the build context):
#   docker build ... --build-arg AG_BUILD_COMMIT=$(git rev-parse HEAD) --build-arg AG_BUILD_DATE=$(git show -s --format=%cs HEAD)
# so that `ag-server --version` can say which commit the image was built from.

ARG RUST_VERSION=1.89
ARG CARGO_CHEF_VERSION=0.1.78

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

# ---- compile ---------------------------------------------------------------------------------------
FROM chef AS build
# --locked: the lockfile decides every version. Paths are remapped so the binary does not embed the
# build directory (a step towards reproducible builds). The same flags for cook and build, or the
# cooked dependencies would be rebuilt.
ENV RUSTFLAGS="--remap-path-prefix=/src=/agentgit --remap-path-prefix=/usr/local/cargo=/cargo" \
    CARGO_INCREMENTAL=0
COPY --from=planner /src/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo chef cook --release --locked --recipe-path recipe.json -p ag-server -p ag-cli
COPY . .
ARG AG_BUILD_COMMIT=""
ARG AG_BUILD_DATE=""
ENV AG_BUILD_COMMIT=${AG_BUILD_COMMIT} \
    AG_BUILD_DATE=${AG_BUILD_DATE}
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --locked -p ag-server -p ag-cli \
 && install -D target/release/ag-server /out/ag-server \
 && install -D target/release/ag /out/ag \
 && install -d -o 65532 -g 65532 /out/data

# ---- the command-line client ------------------------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot AS cli
LABEL org.opencontainers.image.title="agentgit-cli" \
      org.opencontainers.image.description="The ag command-line client" \
      org.opencontainers.image.source="https://github.com/Paffin/AgentGit" \
      org.opencontainers.image.licenses="Apache-2.0"
COPY --from=build /out/ag /usr/local/bin/ag
COPY LICENSE NOTICE /licenses/
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
ENV AG_SERVER_CONFIG=/etc/agentgit/ag-server.toml
USER 65532:65532
VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=15s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/ag-server", "healthcheck", "--url", "http://127.0.0.1:8080"]
ENTRYPOINT ["/usr/local/bin/ag-server"]
CMD ["serve"]
