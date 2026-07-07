# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.x (current) | Yes — actively maintained |

Only the latest published crate version receives security fixes.
Older versions are not patched; please upgrade.

## Reporting a Vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Report vulnerabilities through GitHub private security advisories:

<https://github.com/elioetibr/skald/security/advisories/new>

You will receive an acknowledgement within **2 business days** and a
status update (accepted, declined, or in progress) within **7 calendar
days**.  If a fix is warranted the maintainers will coordinate a release
under an embargo and credit the reporter in the advisory unless
anonymity is requested.

## Security Posture

### Memory safety

- `skald-core`, `skald-ast`, and `skald-cst` carry `#![forbid(unsafe_code)]`.
  Zero `unsafe` blocks in the library surface.
- Miri runs in CI on every nightly build (`nightly.yml`, `miri` job) with
  `-Zmiri-strict-provenance` to catch stacked-borrows and provenance violations.

### Resource-exhaustion protection

`ResourceLimits` is applied by default at every entry point and guards against:

- Billion-laughs / alias expansion storms (`max_alias_expansions`).
- Deep nesting / stack exhaustion (`max_nesting_depth`).
- Oversized documents (`max_document_size_bytes`).
- Excessive node counts (`max_nodes`).

All limits are tunable; the defaults are chosen conservatively so that
untrusted input is safe to parse without caller configuration.

### Tag safety

YAML tags are surfaced as data (`Node` metadata) and are never executed or
resolved to external schemas.  There is no tag-URI fetching.

### Supply-chain controls

See [`docs/supply-chain.md`](docs/supply-chain.md) for the full control matrix.
Key highlights:

- `cargo audit` + `cargo deny` run on every PR (`supply-chain` job in
  `ci.yml`).
- `cargo vet` audits are tracked in `supply-chain/audits.toml`
  (`vet` job in `ci.yml`).
- Release artifacts carry SLSA Level 3 provenance and are signed with
  sigstore/cosign keyless signing (`release.yml`).
- OpenSSF Scorecard runs weekly (`scorecard.yml`).
- REUSE 3.3 license compliance is declared in `REUSE.toml`; every file in the
  repository is covered.
- 8 cargo-fuzz targets exercise the full parsing pipeline on every nightly run.
