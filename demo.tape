# The README GIF (spec §12.1 #18). CI renders it from the synthetic demo root:
#   ROLLCALL_MATERIALIZE_DIR=<dir> cargo test -p rollcall-cli --test it --locked -- --ignored materialize_demo
#   ROLLCALL_DEMO=<dir>/demo vhs demo.tape     (with a built `rollcall` on PATH)
# `claude` and `codex` are stubs that print the session they were asked to resume.

Output demo.gif
Output demo.txt

Set Shell bash
Set FontSize 14
Set Width 1400
Set Height 600
Set Padding 16
Set TypingSpeed 45ms

Hide
Type `s=$(mktemp -d) && printf '#!/bin/sh\necho "resumed $2"\n' > "$s/claude" && cp "$s/claude" "$s/codex" && chmod +x "$s/claude" "$s/codex"`
Enter
Type `export PATH="$s:$PATH" HOME="$(mktemp -d)" CLAUDE_CONFIG_DIR="$ROLLCALL_DEMO/.agents/claude" CODEX_HOME="$ROLLCALL_DEMO/.agents/codex" ROLLCALL_NOW=2026-09-28T12:00:00Z PS1='$ '`
Enter
Type `cd "$ROLLCALL_DEMO" && clear`
Enter
Show

Type "rollcall status"
Sleep 300ms
Enter
Sleep 3s

Type "rollcall"
Sleep 300ms
Enter
Sleep 2s
Down
Sleep 1s
Down
Sleep 1s
Up
Sleep 1s
Up
Sleep 1.5s
Type "R"
Sleep 2s
