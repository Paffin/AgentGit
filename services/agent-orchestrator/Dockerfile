# syntax=docker/dockerfile:1
#
# Agent Platform (orchestrator) image.
#
# Build from this directory:
#     docker build -t agentgit/agent-orchestrator:dev .
#
# NOTE: this Dockerfile has not been built by its authors (the machine it was
# written on has a Docker client but no daemon). The Go build it runs is the one
# the tests exercise; the image-specific parts (base image names, the nonroot
# user, ownership of the state directory) are standard but unverified.

FROM golang:1.27 AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY . .
# A static binary: no cgo, no libc, nothing to install in the final image.
ARG VERSION=dev
RUN CGO_ENABLED=0 go build -trimpath \
      -ldflags "-s -w -X github.com/Paffin/AgentGit/services/agent-orchestrator/internal/app.Version=${VERSION}" \
      -o /out/orchestrator ./cmd/orchestrator \
 && mkdir /out/state

# distroless "static" holds CA certificates and a nonroot user (uid 65532) and
# nothing else: no shell, no package manager. Command executors therefore need
# their programs added to a derived image (or run the orchestrator elsewhere).
FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /out/orchestrator /orchestrator
# Agent keys, the semantic index and session work directories live under the
# state directory (set state_dir: /var/lib/orchestrator). Created here, owned by
# the nonroot user, so that an empty volume mounted over it is writable.
COPY --from=build --chown=65532:65532 /out/state /var/lib/orchestrator
VOLUME ["/var/lib/orchestrator"]
# The admin API. It listens on 127.0.0.1 unless the configuration says
# otherwise; to reach it from outside the container set listen: 0.0.0.0:9090
# AND admin.token_env (the service refuses a non-loopback listener without one).
EXPOSE 9090
USER 65532:65532
ENTRYPOINT ["/orchestrator"]
CMD ["run", "-config", "/etc/orchestrator/orchestrator.yaml"]
