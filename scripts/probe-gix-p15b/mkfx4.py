import os, shutil, struct, sys, zlib

root, n = sys.argv[1], int(sys.argv[2])
src = os.path.join(root, "refdelta")
dst = os.path.join(root, f"ofschain{n}")
if os.path.exists(dst):
    shutil.rmtree(dst)
shutil.copytree(src, dst)
g = os.path.join(dst, ".git")
pack_dir = os.path.join(g, "objects/pack")
for f in os.listdir(pack_dir):
    os.remove(os.path.join(pack_dir, f))
X = bytes([0x11] * 20)
blob = bytes([0x30]) + zlib.compress(b"")  # non-delta blob, size 0, at offset 12
assert len(blob) == 9
first = bytes([0x60, len(blob)])  # ofs-delta, size 0, distance = len(blob) -> the blob
rest = b"\x60\x02" * (n - 1)  # each entry is 2 bytes, base is the entry 2 bytes before
body = blob + first + rest
last_offset = 12 + len(blob) + 2 * (n - 1)
pack = b"PACK" + struct.pack(">II", 2, n + 1) + body + bytes([0x22] * 20)
name = "pack-" + "ab" * 20
with open(os.path.join(pack_dir, name + ".pack"), "wb") as f:
    f.write(pack)
idx = b"\xfftOc" + struct.pack(">I", 2)
idx += b"".join(struct.pack(">I", 1 if b >= X[0] else 0) for b in range(256))
idx += X + struct.pack(">I", 0) + struct.pack(">I", last_offset)
idx += bytes([0x22] * 20) + bytes([0x33] * 20)
with open(os.path.join(pack_dir, name + ".idx"), "wb") as f:
    f.write(idx)
print(f"ofschain{n}", len(pack), "bytes")
