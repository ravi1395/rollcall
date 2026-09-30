import hashlib, os, struct, sys, zlib

root = sys.argv[1]
EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"


def base(name, extra_config="", head_hex=None, write_commit=True):
    d = os.path.join(root, name)
    g = os.path.join(d, ".git")
    for sub in ("objects/pack", "objects/info", "refs/heads"):
        os.makedirs(os.path.join(g, sub), exist_ok=True)
    body = (
        f"tree {EMPTY_TREE}\nauthor A <a@b> 0 +0000\ncommitter A <a@b> 0 +0000\n\nm\n"
    ).encode()
    raw = b"commit %d\0" % len(body) + body
    cid = hashlib.sha1(raw).hexdigest()
    if head_hex:
        cid = head_hex
    elif write_commit:
        od = os.path.join(g, "objects", cid[:2])
        os.makedirs(od, exist_ok=True)
        with open(os.path.join(od, cid[2:]), "wb") as f:
            f.write(zlib.compress(raw))
    with open(os.path.join(g, "HEAD"), "w") as f:
        f.write("ref: refs/heads/main\n")
    with open(os.path.join(g, "refs/heads/main"), "w") as f:
        f.write(cid + "\n")
    with open(os.path.join(g, "config"), "w") as f:
        f.write("[core]\n\trepositoryformatversion = 0\n\tbare = true\n" + extra_config)
    return d, g, cid


# 1. shallowFile -> /dev/zero
base("shallow-zero", '[gitoxide "core"]\n\tshallowFile = /dev/zero\n')

# 2. shallowFile -> FIFO inside the tree
d, g, _ = base("shallow-fifo", '[gitoxide "core"]\n\tshallowFile = fifo\n')
os.mkfifo(os.path.join(g, "fifo"))

# 3. indexFile redirect (and a replaceRefBase one)
base("indexpath", '[gitoxide "core"]\n\tindexFile = /dev/zero\n')

# 4. replaceRefBase pointing at objects/pack/, holding a 256 MiB file
d, g, _ = base(
    "replace",
    '[core]\n\tuseReplaceRefs = false\n[gitoxide "objects"]\n\treplaceRefBase = objects/pack/\n',
)
with open(os.path.join(g, "objects/pack/big"), "wb") as f:
    f.truncate(256 * 1024 * 1024)

# 5. ref-delta whose base is itself
X = bytes([0x11] * 20)
d, g, _ = base("refdelta", head_hex=X.hex())
entry = bytes([0x70]) + X + zlib.compress(b"")
pack = b"PACK" + struct.pack(">II", 2, 1) + entry
trailer = bytes([0x22] * 20)
pack += trailer
idx = b"\xfftOc" + struct.pack(">I", 2)
fan = [0] * 256
for b in range(256):
    fan[b] = 1 if b >= X[0] else 0
idx += b"".join(struct.pack(">I", v) for v in fan)
idx += X
idx += struct.pack(">I", zlib.crc32(entry))
idx += struct.pack(">I", 12)
idx += trailer
idx += bytes([0x33] * 20)
name = "pack-" + "ab" * 20
with open(os.path.join(g, "objects/pack", name + ".pack"), "wb") as f:
    f.write(pack)
with open(os.path.join(g, "objects/pack", name + ".idx"), "wb") as f:
    f.write(idx)

# 6. control: plain repo
base("control")
print("ok")
