# syntax=docker/dockerfile:1.7@sha256:a57df69d0ea827fb7266491f2813635de6f17269be881f696fbfdf2d83dda33e

# Multi-architecture manifest digests are release inputs. Update them only
# together with scripts/test-container-supply-chain-policy.sh and a complete
# amd64/arm64 container rehearsal.

FROM node:24-bookworm-slim@sha256:6f7b03f7c2c8e2e784dcf9295400527b9b1270fd37b7e9a7285cf83b6951452d AS gui-builder

WORKDIR /workspace/apps/schemahub-gui

COPY shuozeli/codegen/schemahub/apps/schemahub-gui/package.json \
     shuozeli/codegen/schemahub/apps/schemahub-gui/pnpm-lock.yaml \
     shuozeli/codegen/schemahub/apps/schemahub-gui/pnpm-workspace.yaml \
     ./
RUN corepack enable \
    && corepack prepare pnpm@11.2.2 --activate \
    && pnpm install --frozen-lockfile

COPY shuozeli/codegen/schemahub/apps/schemahub-gui/ ./
RUN pnpm run build \
    && pnpm run test:bundle

FROM rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 AS builder

ARG SCHEMAHUB_VERSION=0.1.0-dev
ARG TARGETARCH
ENV CARGO_INCREMENTAL=0 \
    SCHEMAHUB_VERSION=${SCHEMAHUB_VERSION}

WORKDIR /workspace/shuozeli/codegen/schemahub

RUN --mount=type=cache,id=schemahub-cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=schemahub-cargo-git-${TARGETARCH},target=/usr/local/cargo/git,sharing=locked \
    cargo install cargo-auditable --locked --version 0.7.5 --force

# Both compiler boundaries resolve independently from immutable Cargo Git
# revisions, so the image build needs only the SchemaHub source tree.
COPY shuozeli/codegen/schemahub /workspace/shuozeli/codegen/schemahub

RUN --mount=type=cache,id=schemahub-cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=schemahub-cargo-git-${TARGETARCH},target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=schemahub-target-${TARGETARCH},target=/workspace/shuozeli/codegen/schemahub/target,sharing=locked \
    cargo auditable build \
    --release \
    --locked \
    -p schemahub-server \
    -p schemahub-cli \
    --features schemahub-server/postgres \
    && install -d /runtime-data /runtime-bin \
    && cp target/release/schemahub-server target/release/schemahub /runtime-bin/

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:fccdbb0a547c14e23fcf4ce8ad62ca5d43b4faae8d22cd292f490fef9946c96e AS runtime

ARG SCHEMAHUB_VERSION=0.1.0-dev
ARG VCS_REF=unknown
ARG BUILD_DATE=unknown
ARG PROTOBUF_RS_REVISION=unknown
ARG FLATBUFFERS_RS_REVISION=unknown

LABEL org.opencontainers.image.title="SchemaHub" \
      org.opencontainers.image.description="Human and agent schema collaboration and immutable artifact serving" \
      org.opencontainers.image.source="https://github.com/Shuozeli/schemahub" \
      org.opencontainers.image.version="${SCHEMAHUB_VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.created="${BUILD_DATE}" \
      io.schemahub.protobuf-rs.revision="${PROTOBUF_RS_REVISION}" \
      io.schemahub.flatbuffers-rs.revision="${FLATBUFFERS_RS_REVISION}" \
      io.schemahub.gui.path="/usr/share/schemahub/gui/index.html"

COPY --from=builder \
    /runtime-bin/schemahub-server \
    /usr/local/bin/schemahub-server
COPY --from=builder \
    /runtime-bin/schemahub \
    /usr/local/bin/schemahub
COPY --from=builder --chown=65532:65532 /runtime-data /var/lib/schemahub
COPY --from=gui-builder --chown=65532:65532 \
    /workspace/apps/schemahub-gui/dist \
    /usr/share/schemahub/gui

USER 65532:65532
WORKDIR /var/lib/schemahub
VOLUME ["/var/lib/schemahub"]

EXPOSE 50051 8080
STOPSIGNAL SIGTERM

HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/schemahub-server", "--check-ready", "http://127.0.0.1:8080/readyz"]

ENTRYPOINT ["/usr/local/bin/schemahub-server"]
CMD ["--listen", "0.0.0.0:50051", "--http-listen", "0.0.0.0:8080", "--gui-dir", "/usr/share/schemahub/gui", "--db", "/var/lib/schemahub/schemahub.redb"]
