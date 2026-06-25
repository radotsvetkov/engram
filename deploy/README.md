# Deploying Engram on a cheap VPS

Engram is a single static binary that idles at **0 MB resident** via systemd socket
activation: systemd holds the port, and `engramd` is spawned only when a request
arrives, then exits after `ENGRAM_IDLE_SECS`. On a $5 VPS this is genuinely
pay-for-what-you-use.

## 1. Build the static binary

**With Docker (any host):**

```sh
docker build -f deploy/Dockerfile -t engramd .
docker create --name engram-extract engramd
docker cp engram-extract:/usr/local/bin/engramd ./engramd
docker rm engram-extract
```

**Native on a Linux box:**

```sh
rustup target add x86_64-unknown-linux-musl
sudo apt-get install -y musl-tools
CC_x86_64_unknown_linux_musl=musl-gcc \
  cargo build --release --target x86_64-unknown-linux-musl -p engramd
strip target/x86_64-unknown-linux-musl/release/engramd
```

The default build is offline-only. For a real LLM provider add `--features http`
(this pulls in rustls).

## 2. Install on the VPS

```sh
scp engramd root@VPS:/usr/local/bin/engramd
scp deploy/engram.socket deploy/engram.service root@VPS:/etc/systemd/system/
ssh root@VPS '
  install -d -o root -g root /var/lib/engram
  systemctl daemon-reload
  systemctl enable --now engram.socket
'
```

`engram.service` runs under `DynamicUser` with `ProtectSystem=strict`,
`NoNewPrivileges`, and a private tmp — minimal privilege for a self-modifying agent.
State (the brain, the ledger, the signing keys) lives in `/var/lib/engram`.

## 3. TLS and the public edge (optional)

Keep `engramd` bound to localhost and terminate TLS at a reverse proxy. With Caddy:

```
# /etc/caddy/Caddyfile
your.domain {
    reverse_proxy 127.0.0.1:8088
}
```

Put the dashboard behind auth before exposing it — a self-modifying agent's control
plane is sensitive. Until then, reach it over an SSH tunnel:
`ssh -L 8088:127.0.0.1:8088 root@VPS` then open http://127.0.0.1:8088.

## 4. Scheduled wake

Recurring jobs are stored in the brain. To wake the sleeping core on schedule,
install a systemd timer alongside the socket — `engram-sched`'s `systemd::wake_timer`
generates the `.service`/`.timer` pair, or adapt this:

```
# /etc/systemd/system/engram-wake.timer
[Timer]
OnCalendar=*-*-* 09:00:00
Persistent=true
[Install]
WantedBy=timers.target
```

`Persistent=true` means a fire missed while the box was off runs once on next boot —
matching the scheduler's skip-on-missed policy (one catch-up, never a stampede).

## Footprint

The full agent — hybrid memory + SQLite, the WASM skill sandbox, the gateway, the
scheduler, the audit ledger, and the HTTP server + dashboard — is a **~3 MB** binary
that holds **0 MB resident at idle**, versus an always-on multi-hundred-MB Python/Node
runtime. That is the deployment thesis, and it is why Engram costs nearly nothing when
you are not using it.
