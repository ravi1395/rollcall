# COUNTS — rollcall v0

Actual measured counts after each lane/phase merge on `impl/v0`. Baseline row measured this session before any v0 code landed.

| After | Commit | cargo test (passed / failed / ignored) | clippy -D warnings | fmt --check |
|---|---|---|---|---|
| baseline | 7f6d930 | n/a — no `Cargo.toml` (`could not find Cargo.toml`) | n/a | n/a |
| lanes A, B2 (P01–P04, P05–P06, P11–P13) | a857c5c | 115 / 0 / 0 (core 105, cli 10) | clean | clean |
| lanes C2, C3, B1 P07, C1 P14/P16–P18 | 5c085ea | 377 / 0 / 2 (core 105, git 111, sessions 135, store 16 + 2 ignored, cli 10) | clean | clean |
| lanes C1 P19, B1 P08 (+ `b25517a` test fix) | eb419fc | 402 / 0 / 2 (core 105, git 136, sessions 135, store 16 + 2 ignored, cli 10) | clean | clean |
| lane B1 P07b/P09/P10, C4 P27a | 4866407 | 408 / 0 / 2 (core 105, git 136, sessions 135, store 16 + 2 ignored, cli 10, cli it 6) | clean | clean |
| C1 P18b | ce781ee | 441 / 0 / 2 (git 169) | clean | clean |
| C1 P20 (+ core serde derives, spec rev 13) | 3709e11 | 493 / 0 / 2 (git 219) | clean | clean |
| C4 P28 | 089d79e | 500 / 0 / 4 (cli it 15 + 2 ignored materialize) | clean | clean |
| D P29, C1 P21 | 2f39ccd | 525 / 0 / 4 (git 231, cli lib 23) | clean | clean |
| D P38 | c2730bf | 541 / 0 / 4 (cli lib 39) | clean | clean |
| D P36 (+ spec rev 13 round 1 fixes) | 5b23879+P36 | 558 / 0 / 4 (cli lib 57) | clean | clean |
| D P37 | 0ba28a6 | 578 / 0 / 5 (cli lib 76 + 1 ignored until P31) | clean | clean |
