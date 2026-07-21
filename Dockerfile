# syntax=docker/dockerfile:1.7

ARG RUST_VERSION=1.95.0
FROM rust:${RUST_VERSION}-bookworm AS builder

ARG SCHEMAHUB_VERSION=0.1.0-dev
ARG TARGETARCH
ARG CARGO_AUDITABLE_VERSION=0.7.5
ENV CARGO_INCREMENTAL=0 \
    SCHEMAHUB_VERSION=${SCHEMAHUB_VERSION}

WORKDIR /workspace/shuozeli/codegen/schemahub

RUN --mount=type=cache,id=schemahub-cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=schemahub-cargo-git-${TARGETARCH},target=/usr/local/cargo/git,sharing=locked \
    cargo install cargo-auditable --locked --version ${CARGO_AUDITABLE_VERSION}

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

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

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
      io.schemahub.flatbuffers-rs.revision="${FLATBUFFERS_RS_REVISION}"

COPY --from=builder \
    /runtime-bin/schemahub-server \
    /usr/local/bin/schemahub-server
COPY --from=builder \
    /runtime-bin/schemahub \
    /usr/local/bin/schemahub
COPY --from=builder --chown=65532:65532 /runtime-data /var/lib/schemahub

USER 65532:65532
WORKDIR /var/lib/schemahub
VOLUME ["/var/lib/schemahub"]

EXPOSE 50051 8080
STOPSIGNAL SIGTERM

HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/schemahub-server", "--check-ready", "http://127.0.0.1:8080/readyz"]

ENTRYPOINT ["/usr/local/bin/schemahub-server"]
CMD ["--listen", "0.0.0.0:50051", "--http-listen", "0.0.0.0:8080", "--db", "/var/lib/schemahub/schemahub.redb"]
