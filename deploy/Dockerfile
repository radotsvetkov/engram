# Build a single static musl binary with real model providers, then ship it in a minimal image.
#
#   docker build -f deploy/Dockerfile -t engram .
#   # Reachable, authenticated (required for a non-loopback bind):
#   docker run -d -p 8088:8088 -e ENGRAM_API_TOKEN=$(openssl rand -hex 16) \
#              -e ANTHROPIC_API_KEY=sk-ant-... -v engram-data:/var/lib/engram engram
#
# Interactive browser automation needs Chromium; build that variant from deploy/Dockerfile.browser.

FROM rust:1.94-bookworm AS build
RUN rustup target add x86_64-unknown-linux-musl \
 && apt-get update && apt-get install -y --no-install-recommends musl-tools \
 && rm -rf /var/lib/apt/lists/*
# rusqlite bundles SQLite (C) and rustls' ring needs a C compiler, so the musl target needs musl-gcc.
ENV CC_x86_64_unknown_linux_musl=musl-gcc \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
WORKDIR /src
COPY . .
# --features http ships the real OpenAI-compatible + Anthropic providers (rustls TLS). Without it
# the image would be mock-only - useless for a real deployment.
RUN cargo build --release --target x86_64-unknown-linux-musl -p engramd --features http,docs \
 && strip target/x86_64-unknown-linux-musl/release/engramd

FROM alpine:3.20
RUN adduser -D -H -u 10001 engram && mkdir -p /var/lib/engram && chown engram /var/lib/engram
COPY --from=build /src/target/x86_64-unknown-linux-musl/release/engramd /usr/local/bin/engramd
# Persist the brain (memory DB, signing key, ledger, config) across container recreation.
VOLUME /var/lib/engram
USER engram
ENV ENGRAM_HOME=/var/lib/engram \
    ENGRAM_ADDR=0.0.0.0:8088 \
    ENGRAM_IDLE_SECS=900
# Secure by default: a non-loopback bind refuses to start without ENGRAM_API_TOKEN (the daemon
# enforces this), so an open self-modifying control plane can never be exposed by accident.
EXPOSE 8088
# Liveness probe (busybox wget; /health is always reachable, no auth needed).
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD wget -q -O- http://127.0.0.1:8088/health >/dev/null 2>&1 || exit 1
ENTRYPOINT ["/usr/local/bin/engramd"]
