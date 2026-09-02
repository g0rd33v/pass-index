# Builds the pass-index service: the catalogue store with its HTTP face
# (serve) plus the seed and export binaries for docker exec.
FROM rust:1.97 AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p index --bins

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/serve /usr/local/bin/serve
COPY --from=builder /build/target/release/seed /usr/local/bin/seed
COPY --from=builder /build/target/release/export /usr/local/bin/export
COPY --from=builder /build/target/release/collect /usr/local/bin/collect
COPY --from=builder /build/target/release/mint /usr/local/bin/mint
COPY --from=builder /build/target/release/import /usr/local/bin/import
COPY --from=builder /build/target/release/repair /usr/local/bin/repair
COPY --from=builder /build/target/release/check /usr/local/bin/check
COPY --from=builder /build/target/release/audit /usr/local/bin/audit
COPY --from=builder /build/target/release/exportkb /usr/local/bin/exportkb
COPY --from=builder /build/target/release/resolvecheck /usr/local/bin/resolvecheck
USER 65534:65534
ENTRYPOINT ["/usr/local/bin/serve"]
