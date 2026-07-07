# Supply-Chain Security Posture

This document maps every supply-chain and security control in the Skald
repository to the mechanism that implements it and where it runs.

## Control Matrix

| Control | Mechanism | Local mise run target | CI job / workflow |
|---------|-----------|-------------------|-------------------|
| **Memory safety** | `#![forbid(unsafe_code)]` on skald-core / skald-ast / skald-cst; Miri with `-Zmiri-strict-provenance` | `mise run miri` | `nightly.yml` — `miri` job |
| **Dependency advisories** | `cargo audit` against the RustSec advisory database | `mise run audit` | `ci.yml` — `supply-chain` job |
| **License & ban policy** | `cargo deny check` (licenses allowlist, multiple-versions ban, unknown-registries ban) | `mise run deny` | `ci.yml` — `supply-chain` job |
| **Supply-chain audits** | `cargo vet` — audit records in `supply-chain/audits.toml`; non-blocking until tree is certified | `mise run vet` | `ci.yml` — `vet` job (continue-on-error) |
| **Build provenance** | SLSA Level 3 via `slsa-framework/slsa-github-generator` | CI only | `release.yml` — `provenance` job |
| **Artifact signing** | sigstore/cosign keyless signing (`.cosign.bundle` per artifact) | CI only | `release.yml` — `sign` job |
| **Project scorecard** | OpenSSF Scorecard (`ossf/scorecard-action`); weekly schedule | CI only | `scorecard.yml` — `analysis` job |
| **License compliance** | REUSE 3.3 — every file covered via `REUSE.toml` globs + `LICENSES/` directory | `mise run reuse` | _(run `reuse lint` locally; CI integration pending)_ |
| **Fuzzing** | 8 `cargo-fuzz` targets: `fuzz_scanner`, `fuzz_parser`, `fuzz_round_trip`, `fuzz_limits`, `fuzz_serde`, `fuzz_cst_roundtrip`, `fuzz_lossless_edit`, `fuzz_merge_keys` | `mise run fuzz-all` | `nightly.yml` — `fuzz` job |
| **Test coverage** | `cargo-tarpaulin` with LLVM engine; 100% line coverage required (`--fail-under 100`) | `mise run coverage` | `ci.yml` — `coverage` job (macOS runner) |

## Notes

**Ordering in supply-chain job** — `cargo audit` intentionally runs before
`cargo deny` in `ci.yml`.  `cargo audit` clones the RustSec advisory database
into `~/.cargo/advisory-db`; `cargo deny`'s `advisories` check reuses that
directory and will fail if it is non-empty when the clone is attempted.

**cargo vet is non-blocking** — the `vet` job in `ci.yml` carries
`continue-on-error: true` until the dependency audit set is fully populated
with `cargo vet certify` entries.  Remove that flag once the tree is audited.

**REUSE compliance** — `REUSE.toml` uses glob patterns to cover every file
class (Rust sources carry SPDX headers inline; config files, docs, and CI
workflows are covered by catch-all globs).  Run `reuse lint` or `mise run reuse`
to verify compliance locally.

**SLSA / cosign are release-only** — these controls run on tag pushes
(`on: push: tags: ['v*']`) and are not part of the PR gate.  They operate on
the published `.tar.gz` / `.sha256` release assets.

**Miri scope** — Miri tests `skald-core` and `skald-ast`.  `skald-serde` and
`skald-cst` pull in `serde` proc-macros which Miri does not yet fully support;
they are intentionally excluded.
