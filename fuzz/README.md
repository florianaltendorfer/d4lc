# Fuzzing the OCR text conversion path

cargo-fuzz targets that feed OCR-style garbage text into the `d4lc-parse`
conversion entry points (affix, aspect, unique, rune, sigil, item type and
power, and the `TextToAffixValue` number extraction) with real enUS fuzzy
indexes. The contract under test: every conversion returns for any UTF-8
input without panicking. libFuzzer records any panic, arithmetic overflow,
or abort as a crash.

This crate is a dev-machine-only tool. It is not a member of the parent
workspace, it is never built or run by `scripts/verify_local.sh`, CI, or the
live OCR hot path, and it requires a nightly toolchain.

## Layout

```
fuzz/
├── Cargo.toml            # standalone crate; empty [workspace] keeps it out of the parent
├── fuzz_targets/         # one libFuzzer entry point per file
├── seeds/                # committed seed inputs taken from existing OCR test fixtures
├── corpus/               # grown corpus (gitignored, created on demand)
└── artifacts/            # crash inputs (gitignored)
```

## Prerequisites (once)

```sh
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz --locked
```

## Build

```sh
cargo +nightly fuzz build
```

## Run a short smoke session per target

```sh
cargo +nightly fuzz run parse_text_value corpus/parse_text_value seeds/parse_text_value -- -max_total_time=30
cargo +nightly fuzz run parse_conversions_cached corpus/parse_conversions_cached seeds/parse_conversions_cached -- -max_total_time=30 -max_len=512
cargo +nightly fuzz run parse_rune_sigil corpus/parse_rune_sigil seeds/parse_rune_sigil -- -max_total_time=30 -max_len=512
```

Run these from this `fuzz/` directory. New inputs are written into the first
corpus directory, so list `corpus/<target>` before `seeds/<target>` to keep
the committed seed files unchanged. The corpus directory must exist before
the run (`mkdir -p corpus/<target>`); the seeds directory is committed.
- `-max_len=512` bounds the input length for the index-backed targets: each
  execution runs full fuzzy scans over hundreds of candidates, and OCR
  tooltip lines are short, so longer inputs mostly cost time without adding
  coverage. It also keeps the upstream-parity match cache bounded during
  long local sessions.
- For deeper sessions, raise `-max_total_time` (or drop it for an endless
  run) and consider `-jobs=$(nproc)`.

## Targets

| Target | Entry points exercised |
| --- | --- |
| `parse_text_value` | `text_to_affix_value`, `convert_to_item_power` (no fuzzy indexes needed) |
| `parse_conversions_cached` | `convert_to_affix`, `convert_to_affix_multiline`, `convert_to_aspect`, `convert_to_unique`, `convert_to_item_type` |
| `parse_rune_sigil` | `convert_to_rune`, `convert_to_sigil` |

## When a crash is found

libFuzzer writes the offending input to `fuzz/artifacts/<target>/` and prints
a stack trace. Reproduce it without the fuzzer by piping the file into a
target:

```sh
cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<crash-file>
```

Then minimize the input (`-minimize_crash=1`), fix the panic in
`crates/d4lc-parse` or `crates/d4lc-fuzzy`, and add a focused regression test
next to the existing parser tests.
