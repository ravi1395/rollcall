# COUNTS — rollcall v0

Actual measured counts after each lane/phase merge on `impl/v0`. Baseline row measured this session before any v0 code landed.

| After | Commit | cargo test (passed / failed / ignored) | clippy -D warnings | fmt --check |
|---|---|---|---|---|
| baseline | 7f6d930 | n/a — no `Cargo.toml` (`could not find Cargo.toml`) | n/a | n/a |
| lanes A, B2 (P01–P04, P05–P06, P11–P13) | a857c5c | 115 / 0 / 0 (core 105, cli 10) | clean | clean |
