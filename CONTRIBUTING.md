# Contributing to Engram

Thanks for taking the time to look at Engram. Contributions of all sizes are welcome —
bug reports, docs fixes, new skills, and features alike.

## Getting set up

You need a stable Rust toolchain (see [`rust-toolchain.toml`](./rust-toolchain.toml)). Then:

```sh
git clone https://github.com/radotsvetkov/engram.git
cd engram
cargo build
cargo test --workspace
```

Everything builds and tests offline — no network and no API key required. The optional
features (`http` for a real provider, `docs` for document ingest, `browser-cdp` for the
interactive browser) are opt-in.

A `Makefile` wraps the common tasks:

```sh
make run     # start the daemon
make check   # fmt + clippy + tests + the eval suite (what CI runs)
make bench   # the recall + footprint benchmark
```

## Before you open a pull request

Please run the same gate CI does:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p engram-eval
```

- Keep changes focused; one logical change per pull request is easiest to review.
- Match the style of the surrounding code — comment density, naming, and idiom.
- If you change agent behaviour, add or update an [`engram-eval`](./crates/engram-eval)
  case so the change is pinned by a deterministic test rather than a hunch.
- Update the relevant docs when behaviour or configuration changes.

## Adding a skill

Skills live in [`crates/engramd/src/skills`](./crates/engramd/src/skills) and are seeded from
a data-driven table. A good skill is small, keyless where possible, and does one thing well.
Include a clear stdin/stdout contract in the file's docstring and a couple of example inputs.

## Reporting bugs and requesting features

Open an issue using the templates. For anything security-sensitive, please **do not** open a
public issue — follow [`SECURITY.md`](./SECURITY.md) instead.

## Guiding principle

The smallest design that delivers the capability wins. Capability should come from the right
primitives and isolation boundaries, not from more lines of code.

## License

By contributing, you agree that your contributions will be licensed under the
[MIT License](./LICENSE).
