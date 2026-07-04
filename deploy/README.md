# Deploying Engram on a cheap VPS

Engram is a single static binary that idles at **0 MB resident** via systemd socket
activation: systemd holds the port, and `engramd` is spawned only when a request
arrives, then exits after `ENGRAM_IDLE_SECS`. On a $5 VPS this is genuinely
pay-for-what-you-use.

## 1. Get the binary onto the VPS

**Prebuilt (fastest — no toolchain, and it's the exact binary the release CI produces,
already built with the `http,docs` features for a real model provider)**:

```sh
ssh root@VPS 'curl -fsSL https://raw.githubusercontent.com/radotsvetkov/engram/main/install.sh \
  | ENGRAM_INSTALL_DIR=/usr/local/bin sh'
```

The installer resolves the latest release, verifies its published checksum, and drops
`engramd` straight into `/usr/local/bin` (where the systemd unit below expects it).

**Or build it yourself** — with Docker (any host):

```sh
docker build -f deploy/Dockerfile -t engramd .
docker create --name engram-extract engramd
docker cp engram-extract:/usr/local/bin/engramd ./engramd
docker rm engram-extract
scp engramd root@VPS:/usr/local/bin/engramd
```

or natively on a Linux box:

```sh
rustup target add x86_64-unknown-linux-musl
sudo apt-get install -y musl-tools
CC_x86_64_unknown_linux_musl=musl-gcc \
  cargo build --release --target x86_64-unknown-linux-musl -p engramd --features http,docs
strip target/x86_64-unknown-linux-musl/release/engramd
scp target/x86_64-unknown-linux-musl/release/engramd root@VPS:/usr/local/bin/engramd
```

(the default build with no `--features` is offline-only — fine for kicking the tires,
not for a real deployment).

## 2. Install the systemd units

```sh
scp deploy/engram.socket deploy/engram.service root@VPS:/etc/systemd/system/
ssh root@VPS '
  install -d -o root -g root /var/lib/engram
  systemctl daemon-reload
  systemctl enable --now engram.socket
'
```

`engram.service` runs under `DynamicUser` with `ProtectSystem=strict`,
`NoNewPrivileges`, and a private tmp - minimal privilege for a self-modifying agent.
State (the brain, the ledger, the signing keys) lives in `/var/lib/engram`.

## 3. TLS and the public edge (optional)

Keep `engramd` bound to localhost and terminate TLS at a reverse proxy. With Caddy:

```
# /etc/caddy/Caddyfile
your.domain {
    reverse_proxy 127.0.0.1:8088
}
```

Put the dashboard behind auth before exposing it - a self-modifying agent's control
plane is sensitive. Until then, reach it over an SSH tunnel:
`ssh -L 8088:127.0.0.1:8088 root@VPS` then open http://127.0.0.1:8088.

## 4. Scheduled wake

Recurring jobs are stored in the brain. To wake the sleeping core on schedule,
install a systemd timer alongside the socket - `engram-sched`'s `systemd::wake_timer`
generates the `.service`/`.timer` pair, or adapt this:

```
# /etc/systemd/system/engram-wake.timer
[Timer]
OnCalendar=*-*-* 09:00:00
Persistent=true
[Install]
WantedBy=timers.target
```

`Persistent=true` means a fire missed while the box was off runs once on next boot -
matching the scheduler's skip-on-missed policy (one catch-up, never a stampede).

The `OnCalendar` above is a fixed daily poll. For **precise zero-idle wakes** (fire exactly when
the next job is due, not once a day), drive a one-shot timer from `engramd --next-wake`, which prints
the soonest job's epoch-millis (exit 0), or exits 1 when nothing is scheduled — read-only, it never
binds the socket, so run it while the core is asleep:

```sh
# after each run, (re)arm a one-shot timer for the next job:
ms=$(engramd --next-wake) && [ -n "$ms" ] && \
  systemd-run --on-calendar="$(date -u -d @"$((ms/1000))" +%Y-%m-%d\ %H:%M:%S)" \
    --unit=engram-next-wake engramd --run-due
```

`engramd --run-due` fires any due jobs and exits without binding the socket (systemd owns it), so it
composes with the socket-activation unit above.

## Footprint

The full agent - hybrid memory + SQLite, the WASM skill sandbox, the gateway, the
scheduler, the audit ledger, and the HTTP server + dashboard - is a **~3 MB** binary
that holds **0 MB resident at idle**, versus an always-on multi-hundred-MB Python/Node
runtime. That is the deployment thesis, and it is why Engram costs nearly nothing when
you are not using it.
