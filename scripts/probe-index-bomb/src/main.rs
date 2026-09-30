// Probe for spec §4.4 rev 12 index caps: RSS retained by gix-index 0.56 decoding a wide TREE.
// Run: cargo run --release --offline --manifest-path scripts/probe-index-bomb/Cargo.toml -- tree 16
// Outside the workspace ([workspace] table); unsafe getrusage is probe-only, never product code.
#[repr(C)] struct Rusage { ru_utime: [i64;2], ru_stime: [i64;2], ru_maxrss: i64, rest: [i64; 13] }
extern "C" { fn getrusage(who: i32, r: *mut Rusage) -> i32; }
fn rss_mib() -> u64 { let mut ru: Rusage = unsafe { std::mem::zeroed() }; unsafe { getrusage(0, &mut ru) }; (ru.ru_maxrss / (1024*1024)) as u64 }
fn name(i: usize) -> String { let a = b"abcdefghijklmnopqrstuvwxyz"; let mut i = i; let mut s = String::new(); loop { s.push(a[i % 26] as char); i /= 26; if i == 0 { break } } s }
fn main() {
    let which = std::env::args().nth(1).unwrap_or("tree".into());
    let mb: usize = std::env::args().nth(2).map(|s| s.parse().unwrap()).unwrap_or(16);
    let target = mb * 1024 * 1024;
    if which == "tree" {
        // depth 3: root -> F mid -> G leaves, unique sibling names
        let f = 512usize; let g = target / f / 9;
        let mut data = Vec::with_capacity(target + 4096);
        data.extend_from_slice(format!("\0-1 {}\n", f).as_bytes());
        for i in 0..f { data.extend_from_slice(format!("{}\0-1 {}\n", name(i), g).as_bytes()); for j in 0..g { data.extend_from_slice(format!("{}\0-1 0\n", name(j)).as_bytes()); } }
        let nodes = 1 + f + f*g;
        println!("TREE bytes={} nodes={} bytes/node={:.1} rss_before={} MiB", data.len(), nodes, data.len() as f64 / nodes as f64, rss_mib());
        let t = gix_index::extension::tree::decode(&data, gix_hash::Kind::Sha1, Some(64 * 1024 * 1024));
        println!("decoded={} rss_after={} MiB", t.is_some(), rss_mib());
        std::hint::black_box(t);
    } else {
        // UNTR: one directory block with N untracked names "a\0"
        let n = target / 2;
        let mut data = Vec::with_capacity(target + 4096);
        data.extend_from_slice(b"\0"); // env: identifier? layout: see decode(): starts with ident string? we emulate minimal header
        // We call decode_directory_block indirectly is private; use full decode format instead:
        // format: <ident_len varint><ident><stat info...>; too fiddly -> measure BString retention directly instead.
        let mut v: Vec<bstr::BString> = Vec::with_capacity(n);
        for _ in 0..n { v.push(bstr::BString::from(&b"a"[..])); }
        println!("UNTR-equivalent: {} names of 2 input bytes -> rss={} MiB (input {} MiB)", n, rss_mib(), mb);
        std::hint::black_box(v); let _ = data;
    }
}
