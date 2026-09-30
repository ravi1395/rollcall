#!/usr/bin/env python3
"""Print index extension sizes (TREE, UNTR, REUC, ...) for real git indexes.

Usage: probe-index-ext-sizes.py <path/to/.git/index>...
Used to size spec §4.4 revision 12 index-validation caps. Stdlib only.
"""
import struct
import sys


def exts(path):
    d = open(path, "rb").read()
    if d[:4] != b"DIRC":
        return None
    ver, n = struct.unpack(">II", d[4:12])
    hl = 20  # SHA-1 repos only
    off = 12
    prev = b""
    for _ in range(n):
        flags = struct.unpack(">H", d[off + 60:off + 62])[0]
        fixed = 62 + (2 if ver >= 3 and flags & 0x4000 else 0)
        if ver == 4:
            p = off + fixed
            strip, shift = 0, 0
            b = d[p]; p += 1; strip = b & 0x7F
            while b & 0x80:  # git's offset varint
                b = d[p]; p += 1; strip = ((strip + 1) << 7) | (b & 0x7F)
            end = d.index(b"\0", p)
            prev = prev[:len(prev) - strip] + d[p:end]
            off = end + 1
        else:
            end = d.index(b"\0", off + fixed)
            off = off + ((end - off) // 8 + 1) * 8
    out = {}
    while off + 8 <= len(d) - hl:
        sig, size = d[off:off + 4].decode("latin1"), struct.unpack(">I", d[off + 4:off + 8])[0]
        out[sig] = out.get(sig, 0) + size
        off += 8 + size
    return ver, n, out


for p in sys.argv[1:]:
    try:
        r = exts(p)
    except Exception as e:  # probe: report and continue
        print(f"{p}: error {e}")
        continue
    if r:
        ver, n, out = r
        print(f"{p}: v{ver} entries={n} " + " ".join(f"{k}={v}" for k, v in sorted(out.items())))
