# probe-gix-p15b

P15b audit probes (spec revision 13, U-10 .. U-14), against gix 0.88.0 with rollcall's open options. Separate crate (own `[workspace]`), never part of the workspace gates.

- `mkfx*.py <dir>`: build the bare fixtures (`refdelta`, `ofschain8000000`, `shallow-*`, `indexpath`, `replace*`, `treebomb*`).
- `CARGO_TARGET_DIR=<scratch> cargo build`; `probe <mode> <fixture>/.git <secs>` with mode `walk | shallow | tree | headid | indexpath | replace | header`.
- Growth rate (U-13), measured 2026-09-30 on macOS with a **debug** build: `probe headid refdelta/.git 30` sampled by `/bin/ps -o rss=` each second: 199,936 / 386,384 / 515,200 / 594,704 / 711,280 KiB. A `--release` build grows about 17× faster (rev 13 design review: 328 → 700 → 1,057 MiB at 100 ms ticks; at most 42 MiB per 10 ms tick). Use `--release` for figures that size the memory monitor.
- `sample.py <mode> <fixture>/.git <secs> <tick> <probe-binary>`: samples `/bin/ps -o rss=` every tick; prints peak, max growth per tick, first time over 384 MiB (the cap was later set to 320 MiB).
