# Troubleshooting Guide

Common issues when working with the Skald YAML library, organized by symptom.

## Parse Failures

### "unexpected token" on valid YAML

**Symptom:** `ErrorKind::UnexpectedToken` on input that other parsers accept.

**Common causes:**

1. **Indentation mismatch** — Skald is strict about indentation. Mixed tabs and spaces will fail.
2. **Duplicate keys** — Skald defaults to `Strictness::Strict`, which rejects duplicate keys. Use `Strictness::Lenient` to allow them.
3. **Document boundary** — Content after `...` requires `---` to start a new document (YAML 9.2).

**Diagnosis:**

```rust
// Isolate to scanner level
let mut scanner = skald_core::scanner::Scanner::new(input);
while let Some(result) = scanner.next_token() {
    match result {
        Ok(token) => println!("{:?}", token),
        Err(e) => { println!("Scanner error: {e}"); break; }
    }
}
```

### "depth exceeded" or "node count exceeded"

**Symptom:** `ErrorKind::LimitExceeded` on deeply nested or large documents.

**Fix:** Increase `ResourceLimits` for trusted input:

```rust
use skald_core::error::ParserConfig;
use skald_core::limits::ResourceLimits;

let config = ParserConfig {
    limits: ResourceLimits {
        max_depth: 512,           // default: 128
        max_node_count: 10_000_000, // default: 1_000_000
        ..ResourceLimits::default()
    },
    ..ParserConfig::default()
};
```

### Block scalar content is wrong

**Symptom:** Literal (`|`) or folded (`>`) scalars have missing or extra newlines.

**Check:**

1. **Chomping indicator**: `|` (clip, default), `|-` (strip trailing), `|+` (keep trailing)
2. **Indentation indicator**: `|2` forces 2-space indent detection
3. **EOF handling**: Skald treats EOF as an implicit line break (YAML spec)

### Anchor/alias not resolving

**Symptom:** Alias `*name` fails with "undefined alias" error.

**Check:**

1. Anchors are per-document — they reset at `---` boundaries.
2. Forward references are not supported (anchor must appear before alias).
3. Alias expansion count is limited by `max_alias_expansions` (default: 1,024).

## Wrong Output

### Emitter produces different YAML than input

**Expected behavior.** Skald normalizes output:

- Indentation is standardized to `EmitterConfig.indent` (default: 2)
- Flow collections may be expanded to block style
- Comments are not preserved (YAML spec does not require comment preservation)
- Quoted strings may change style based on content

For round-trip fidelity, use the Node API which preserves `ScalarStyle`.

### Serde serialization adds unexpected quotes

**Cause:** The serializer's `needs_quoting()` function in `ser.rs` decides when to quote. A value is quoted if it looks like a YAML special value (true, false, null, numbers) or contains characters that require quoting (`:`, `#`, `[`, `{`, etc.).

**This is correct behavior** — it ensures the output is valid YAML that parses back to the same type.

## Performance Issues

### Benchmarks show regression

**Diagnosis:**

```bash
# Run benchmarks and compare
cargo bench -p skald-bench

# Check specific stage
cargo bench -p skald-bench --bench scanner
cargo bench -p skald-bench --bench parser
cargo bench -p skald-bench --bench composer
cargo bench -p skald-bench --bench emitter
```

**Common causes:**

1. **Extra allocations** — `String::new()` or `.to_owned()` added to hot paths
2. **Algorithm change** — O(n) operation replaced with O(n^2)
3. **Lost zero-copy** — `Cow::Borrowed` replaced with `Cow::Owned`

### Large documents are disproportionately slow

**Check for O(n^2) patterns:**

1. Token buffer scans — `VecDeque` operations should be O(1)
2. Simple key search — should not scan the entire key list
3. Node tree construction — pre-allocate `Vec` capacity when possible

## Test Suite Failures

### YAML test suite test `XXXX` fails

**Diagnosis:**

```bash
# Read the test data
ls skald-yaml-test-suite/data/XXXX/

# Files:
# in.yaml    — input
# test.event — expected event sequence (optional)
# in.json    — expected JSON output (optional)
# error      — if present, input should fail to parse
```

**Common edge cases by category:**

| Test IDs         | Category                         | Known Difficulty               |
| ---------------- | -------------------------------- | ------------------------------ |
| DK4H, ZXT5       | Flow sequence implicit keys      | Single-line constraint         |
| 5LLU, S98Z, W9L4 | Block scalar indentation         | Over-indented whitespace lines |
| 9KBC, CXX2       | Document start line              | Collections on `---` line      |
| G9HC, H7J7       | Node property indentation        | Properties at n+1              |
| BS4K, KS4U       | Bare document after implicit end | Only `---` starts new doc      |
| JEF9/02          | Block scalar EOF                 | Implicit line break at EOF     |
| MUS6/03          | YAML directive whitespace        | Tab after `%YAML`              |

## Build Issues

### `cargo deny check` fails with "multiple versions"

**Cause:** A new dependency pulled in a second version of an existing crate.

**Fix:** Check which dependency introduced the duplicate:

```bash
cargo tree --duplicates
```

Then either pin versions or add a skip entry to `deny.toml` (last resort).

### Clippy warnings after dependency update

**Cause:** New Clippy version introduced stricter lints.

**Fix:** Address warnings directly. If a lint is inappropriate for this codebase, add it to `clippy.toml` or `#[allow()]` with a comment explaining why.
