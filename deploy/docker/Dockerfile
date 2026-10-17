# syntax=docker/dockerfile:1.7
#
# AgentGit images. Build from the repository root:
#
#   docker build -f deploy/docker/Dockerfile -t agentgit-server .                 # the server (default)
#   docker build -f deploy/docker/Dockerfile --target cli -t agentgit-cli .       # the `ag` CLI
#
# Both are static-ish glibc binaries on a distroless base: no shell, no package
# manager, a non-root user (uid 65532). Nothing in this file has been built in
# CI yet — see docs/operations/deployment.md ("What has been verified").

ARG RUST_VERSION=1.89

FROM rust:${RUST_VERSION}-bookworm AS build
WORKDIR /src
# aws-lc-rs and ring compile C/assembly; the `rust` image carries gcc but not always cmake/clang.
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang \
 && rm -rf /var/lib/apt/lists/*
COPY . .
# --locked: the lockfile decides every version. Paths are remapped so the binary
# does not embed the build directory (a step towards reproducible builds).
ENV RUSTFLAGS="--remap-path-prefix=/src=/agentgit --remap-path-prefix=/usr/local/cargo=/cargo" \
    CARGO_INCREMENTAL=0
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p ag-server -p ag-cli \
 && install -D target/release/ag-server /out/ag-server \
 && install -D target/release/ag /out/ag \
 && install -d -o 65532 -g 65532 /out/data

# ---- the command-line client ------------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot AS cli
COPY --from=build /out/ag /usr/local/bin/ag
COPY LICENSE NOTICE /licenses/
USER 65532:65532
WORKDIR /work
ENTRYPOINT ["/usr/local/bin/ag"]

# ---- the server ---------------------------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot AS server
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
