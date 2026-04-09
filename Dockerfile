FROM docker.io/library/rust:1.88-bookworm AS builder

WORKDIR /workspace
COPY . /workspace

RUN rustup component add rust-src && \
    rustup target add wasm32-unknown-unknown

RUN cargo build --locked --profile=production -p ialp-node

FROM docker.io/library/debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /workspace/target/production/ialp-node /usr/local/bin/ialp-node

EXPOSE 30333 9944 9615
ENTRYPOINT ["/usr/local/bin/ialp-node"]
