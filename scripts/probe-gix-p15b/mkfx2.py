import os, shutil, struct, sys, zlib

root = sys.argv[1]
src = os.path.join(root, "refdelta")
dst = os.path.join(root, "refdelta2")
if os.path.exists(dst):
    shutil.rmtree(dst)
shutil.copytree(src, dst)
g = os.path.join(dst, ".git")
X = bytes([0x11] * 20)
payload = b"\x00\x00"  # valid delta header: base size 0, result size 0
entry = bytes([0x70 | len(payload)]) + X + zlib.compress(payload)
pack = b"PACK" + struct.pack(">II", 2, 1) + entry + bytes([0x22] * 20)
name = "pack-" + "ab" * 20
with open(os.path.join(g, "objects/pack", name + ".pack"), "wb") as f:
    f.write(pack)
idx = b"\xfftOc" + struct.pack(">I", 2)
idx += b"".join(struct.pack(">I", 1 if b >= X[0] else 0) for b in range(256))
idx += X + struct.pack(">I", zlib.crc32(entry)) + struct.pack(">I", 12)
idx += bytes([0x22] * 20) + bytes([0x33] * 20)
with open(os.path.join(g, "objects/pack", name + ".idx"), "wb") as f:
    f.write(idx)
print("ok")
