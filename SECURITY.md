# Security Policy

Engram is a self-modifying agent that operates over personal data, so its security model is
part of the product, not an afterthought. The full threat model — threat by threat, with what
ships today versus what is deferred — lives in
[`docs/THREAT-MODEL.md`](./docs/THREAT-MODEL.md).

## Reporting a vulnerability

Please report security issues **privately**. Do not open a public issue for anything that
could put users at risk.

- Preferred: open a [private security advisory](https://github.com/radotsvetkov/engram/security/advisories/new)
  on GitHub.
- Or email **tsvetkov.rado@gmail.com** with the details and, if possible, a proof of concept.

You can expect an acknowledgement within a few days. I'll work with you on a fix and a
coordinated disclosure, and I'm happy to credit you unless you'd prefer to stay anonymous.

## Scope

Things I especially want to hear about:

- Ways to bypass the **taint boundary** (getting a tainted run to shell out or reach egress).
- Ways to **forge or silently alter** a ledger entry so `engramd verify` still passes.
- **Sandbox escapes** from the WASM skill host or the shell/skill gates.
- **SSRF** or redirect-rebinding past the fetch guard.
- Leaks of secrets (provider keys, the signing key, channel secrets) from disk or the API.

## Known boundaries (not vulnerabilities)

The ledger is **tamper-evident**, not tamper-*proof against a fully compromised host* that
holds the signing key. Hardware-backed keys and external co-signing are planned to close that
gap. This is stated plainly in the threat model, so a report that the signing key can be used
by code already running as the user is expected behaviour, not a finding.

## Supported versions

Engram is pre-1.0 and moves quickly; security fixes land on `main` and in the latest release.
