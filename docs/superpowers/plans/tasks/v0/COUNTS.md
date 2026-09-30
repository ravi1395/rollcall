# COUNTS — rollcall v0

Actual measured counts after each lane/phase merge on `impl/v0`. Baseline row measured this session before any v0 code landed.

| After | Commit | cargo test (passed / failed / ignored) | clippy -D warnings | fmt --check |
|---|---|---|---|---|
| baseline | 7f6d930 | n/a — no `Cargo.toml` (`could not find Cargo.toml`) | n/a | n/a |
| lanes A, B2 (P01–P04, P05–P06, P11–P13) | a857c5c | 115 / 0 / 0 (core 105, cli 10) | clean | clean |
| lanes C2, C3, B1 P07, C1 P14/P16–P18 | 5c085ea | 377 / 0 / 2 (core 105, git 111, sessions 135, store 16 + 2 ignored, cli 10) | clean | clean |
| lanes C1 P19, B1 P08 (+ `b25517a` test fix) | eb419fc | 402 / 0 / 2 (core 105, git 136, sessions 135, store 16 + 2 ignored, cli 10) | clean | clean |
