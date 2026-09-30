set -e
D=$1
rm -rf "$D"
mkdir -p "$D"
cd "$D"
git init -q -b main hostile
cd hostile
git config user.email a@b
git config user.name a
echo a > a.txt
git add a.txt
git commit -q -m one
echo b >> a.txt
git commit -q -am two
cat >> .git/config <<'EOF'
[core]
	useReplaceRefs = false
[gitoxide "core"]
	shallowFile = /dev/zero
	indexFile = /dev/zero
[gitoxide "objects"]
	replaceRefBase = objects/pack/
EOF
mkdir -p .git/objects/pack
dd if=/dev/zero of=.git/objects/pack/big bs=1m count=200 2>/dev/null
cd "$D"
git init -q -b main bigtree
cd bigtree
git config user.email a@b
git config user.name a
BLOB=$(echo a | git hash-object -w --stdin)
export BLOB
python3 - <<'PY' > "$D/list.txt"
import os
for i in range(600000):
    print("100644 blob %s\tf%07d-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" % (os.environ["BLOB"], i))
PY
TREE=$(git mktree < "$D/list.txt")
echo "$TREE" > "$D/tree.hex"
COMMIT=$(git commit-tree -m big "$TREE")
git update-ref refs/heads/main "$COMMIT"
cd "$D"
cp -R bigtree bigtree-packed
cd bigtree-packed
git -c gc.auto=0 repack -q -a -d
ls -la .git/objects/pack | head -5
