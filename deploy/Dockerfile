# Build a single static musl binary, then ship it in a minimal image.
# The default build has no network TLS stack (the `http` LLM provider is opt-in),
# so the static musl link is clean. Enable it with: --features http (adds rustls).

FROM rust:1.94-bookworm AS build
RUN rustup target add x86_64-unknown-linux-musl \
 && apt-get update && apt-get install -y --no-install-recommends musl-tools \
 && rm -rf /var/lib/apt/lists/*
# rusqlite bundles SQLite (C), so the musl target needs a musl C compiler.
ENV CC_x86_64_unknown_linux_musl=musl-gcc \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
WORKDIR /src
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl -p engramd \
 && strip target/x86_64-unknown-linux-musl/release/engramd

FROM alpine:3.20
RUN adduser -D -H -u 10001 engram
COPY --from=build /src/target/x86_64-unknown-linux-musl/release/engramd /usr/local/bin/engramd
USER engram
ENV ENGRAM_HOME=/var/lib/engram \
    ENGRAM_ADDR=0.0.0.0:8088 \
    ENGRAM_IDLE_SECS=900
EXPOSE 8088
ENTRYPOINT ["/usr/local/bin/engramd"]
