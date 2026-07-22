# syntax=docker/dockerfile:1.19.0

ARG RUST_IMAGE=rust:1.97.0-bookworm
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian13:nonroot

FROM ${RUST_IMAGE} AS builder
WORKDIR /source
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY e2e/Cargo.toml e2e/Cargo.lock ./e2e/
COPY e2e/src ./e2e/src
WORKDIR /source/e2e
RUN cargo build --release --locked --bin axum-observability-e2e

FROM ${RUNTIME_IMAGE} AS runtime
COPY --from=builder --chmod=0555 /source/e2e/target/release/axum-observability-e2e /server
USER 65532:65532
ENV PORT=8080
EXPOSE 8080
ENTRYPOINT ["/server"]
