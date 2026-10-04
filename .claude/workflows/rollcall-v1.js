export const meta = {
  name: 'rollcall-v1',
  description: 'rollcall post-v0 work from HANDOFF.md: CI test fix, spec rev 16 draft, then serial Sonnet implement + Opus breaker/skeptic per item',
  whenToUse: 'rollcall impl/v0 after the 2026-10-04 handoff. args.stage = "ci-fix" | "spec" | "implement" (implement needs args.specApproved = true; optional args.items = ["I2", ...])',
  phases: [
    { title: 'CI fix', detail: 'Sonnet fixes the Linux-only breaker3 test; Opus skeptic' },
    { title: 'Spec', detail: 'Investigate gix, Opus drafts spec rev 16 + ACCEPTANCE.md, Opus review; stops for author', model: 'opus' },
    { title: 'Criteria', detail: 'Load fixed acceptance criteria from ACCEPTANCE.md' },
    { title: 'Implement', detail: 'Per item, serial: Sonnet implement, gate, commit', model: 'sonnet' },
    { title: 'Break', detail: 'Per item: Opus breaker, guard, Sonnet fixes', model: 'opus' },
    { title: 'Review', detail: 'Per item: Opus skeptic, Sonnet fix loop', model: 'opus' },
    { title: 'Final', detail: 'Opus skeptic over the whole range' },
  ],
}

// ---- constants -------------------------------------------------------------
const REPO = '/Users/ravichandrasekhar/Projects/rollcall'
const SPEC = 'docs/superpowers/specs/2026-09-29-rollcall-design.md'
const AC_FILE = 'docs/superpowers/plans/tasks/v1/ACCEPTANCE.md'
const RULES = 'docs/superpowers/plans/tasks/v0/WORKER-RULES.md'
const GATE = 'cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked'
const TRAILER = 'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'
const MAX_ROUNDS = 3
const IMPL = { model: 'sonnet' }
const CHEAP = { model: 'sonnet', effort: 'low' }
const BREAKER = { agentType: 'breaker', model: 'opus' }
const SKEPTIC = { agentType: 'skeptical-reviewer', model: 'opus' }
const ORDER = ['I2', 'I1', 'I3', 'I5a', 'I5c', 'I5e', 'I5d', 'I5b', 'I4']

const COMMON = `Repo: ${REPO}, branch impl/v0, main checkout (no worktree). Read HANDOFF.md sections "Author decisions 2026-10-03", "Author design answers 2026-10-04" and "Autonomy granted 2026-10-04" first. Repo CLAUDE.md rules R1-R4 bind you; ${RULES} applies except its worktree and target-dir lines (you work in the main checkout with its own target/). Other Claude sessions may share this branch: never revert, stage or commit changes you did not make. Never push. Never print prompt text from ~/.claude or ~/.codex. Use Bash timeouts of 600000 ms for cargo.`

// ---- schemas ---------------------------------------------------------------
const STR = { type: 'string' }
const STRS = { type: 'array', items: STR }
const IMPL_S = { type: 'object', properties: {
  status: { type: 'string', enum: ['done', 'blocked'] }, files_changed: STRS,
  red_evidence: STR, verification: STR, notes: STR },
  required: ['status', 'files_changed', 'verification'] }
const GATE_S = { type: 'object', properties: { pass: { type: 'boolean' }, test_summary: STR, failures: STRS },
  required: ['pass', 'test_summary', 'failures'] }
const COMMIT_S = { type: 'object', properties: { sha: STR, files: STRS, note: STR }, required: ['sha', 'files'] }
const HEAD_S = { type: 'object', properties: { sha: STR, clean: { type: 'boolean' }, status: STR }, required: ['sha', 'clean', 'status'] }
const BREAK_S = { type: 'object', properties: {
  broken: { type: 'array', items: { type: 'object', properties: { test: STR, file: STR, description: STR }, required: ['test', 'description'] } },
  ambiguous: { type: 'array', items: { type: 'object', properties: { question: STR, detail: STR }, required: ['question'] } },
  files_written: STRS, summary: STR },
  required: ['broken', 'ambiguous', 'files_written', 'summary'] }
const GUARD_S = { type: 'object', properties: { reverted: STRS, kept: STRS, note: STR }, required: ['reverted', 'kept'] }
const VERDICT_S = { type: 'object', properties: {
  criteria: { type: 'array', items: { type: 'object', properties: {
    id: STR, verdict: { type: 'string', enum: ['met', 'unmet', 'unverified'] }, evidence: STR }, required: ['id', 'verdict', 'evidence'] } },
  optional: STRS, pass: { type: 'boolean' } },
  required: ['criteria', 'pass'] }
const AC_S = { type: 'object', properties: { items: { type: 'array', items: { type: 'object', properties: {
  id: STR, title: STR, spec_sections: STR, has_logic: { type: 'boolean' }, files_hint: STRS,
  criteria: { type: 'array', items: { type: 'object', properties: { id: STR, text: STR }, required: ['id', 'text'] } } },
  required: ['id', 'title', 'criteria', 'has_logic'] } } }, required: ['items'] }
const SPEC_S = { type: 'object', properties: { files_changed: STRS, item_ids: STRS, open_questions: STRS, summary: STR },
  required: ['files_changed', 'item_ids', 'open_questions', 'summary'] }

// ---- helpers ---------------------------------------------------------------
const acText = item => item.criteria.map(c => `- ${c.id}: ${c.text}`).join('\n')
const unmet = v => (v ? v.criteria.filter(c => c.verdict !== 'met') : [])

const head = (ph, label) => agent(`${COMMON}\nRun git rev-parse HEAD and git status --short -- ':!.claude' (the pathspec excludes the untracked local .claude/ dir). Edit nothing. Return sha, clean (true iff that status output is empty) and the status text verbatim.`,
  { ...CHEAP, phase: ph, label, schema: HEAD_S })

const gate = (ph, label) => agent(`${COMMON}\nRun the full gate from the repo root and edit nothing:\n${GATE}\nReturn pass (true only if every command exited 0), test_summary (every "test result:" line verbatim, plus the total passed/failed/ignored you add up), and failures (names of failing tests or the first error lines of fmt/clippy).`,
  { ...CHEAP, phase: ph, label, schema: GATE_S })

// The tree is verified clean (outside .claude/) at the start of every stage and item, and work is serial,
// so every changed path belongs to this step: commit them all, by explicit path.
const commit = (ph, label, message) => agent(`${COMMON}\nRun git status --short -- ':!.claude'. Stage every listed path (modified, added, deleted, untracked) by explicit path; never git add -A, git add . or anything under .claude/. If nothing is listed, return the current HEAD sha and note "nothing to commit". The commit message is the text between the two --- lines, verbatim, followed by a blank line and the trailer line:\n---\n${message}\n---\n${TRAILER}\nAfter committing run graphify update . if graphify is on PATH (ignore if absent). Return the new sha and the committed paths.`,
  { ...CHEAP, phase: ph, label, schema: COMMIT_S })

async function fix(ph, item, problem, label) {
  return agent(`${COMMON}\nYou fix item ${item.id} (${item.title}). Acceptance criteria (fixed; do not reinterpret):\n${acText(item)}\nProblem to fix:\n${problem}\nFix product code to satisfy the criteria. Never weaken, delete or edit the assertions of an existing test, including breaker tests; if a test itself contradicts the spec, stop and report status "blocked" with the contradiction. R1: if a fix needs Cargo.toml, Cargo.lock, a mod list or a pinned signature, report "blocked". Run the scoped tests that cover your change and quote their "test result:" lines. Do not commit.`,
    { ...IMPL, phase: ph, label, schema: IMPL_S })
}

async function gateUntilGreen(ph, item) {
  for (let r = 1; r <= MAX_ROUNDS; r++) {
    const g = await gate(ph, `gate:${item.id}:${r}`)
    if (g && g.pass) return g
    const f = await fix(ph, item, `The full gate failed:\n${g ? g.failures.join('\n') : 'gate agent died'}\n${g ? g.test_summary : ''}`, `gatefix:${item.id}:${r}`)
    if (!f || f.status === 'blocked') return null
  }
  return null
}

// ---- stage: ci-fix -----------------------------------------------------------
async function ciFix() {
  phase('CI fix')
  const base = await head('CI fix', 'base')
  if (!base || !base.clean) return { stage: 'ci-fix', stopped: 'working tree not clean', status: base && base.status }
  const item = { id: 'CI', title: 'breaker3 test assumes an over-cap fallback path', criteria: [
    { id: 'CI-1', text: 'Before the change, TMPDIR=/tmp cargo test -p rollcall-cli --test it breaker3_binary_blank_names --locked fails on macOS with the same "last cell not at E + 2 of the over-cap path" assertion CI shows; after it, the same command passes, as does the run with the default TMPDIR.' },
    { id: 'CI-2', text: 'The test asserts its own premise (every blank-name fallback path it checks is wider than 40 display columns) before checking the E + 2 placement, so the premise cannot silently go false on another OS.' },
    { id: 'CI-3', text: 'Only crates/rollcall-cli/tests/it/no_agent.rs (and, if needed, crates/rollcall-cli/tests/it/support.rs) change; no product code, Cargo.toml or Cargo.lock change; no existing assertion is weakened.' },
    { id: 'CI-4', text: 'The full gate passes: ' + GATE },
  ] }
  const impl = await agent(`${COMMON}\nFix the CI failure described in HANDOFF.md "In progress": test no_agent::breaker3_binary_blank_names_and_branches_fall_back_and_trailing_spaces_are_not_counted (crates/rollcall-cli/tests/it/no_agent.rs:396) fails on Linux because /tmp paths are short. Product behaviour is correct per spec §8.2; this is a test-only fix.\nFirst reproduce red on macOS with TMPDIR=/tmp (quote the failing assertion). Preferred fix: build the blank-name repos under a nested directory long enough that every fallback path exceeds 40 display columns on any OS, and assert that premise before the placement check. Acceptance criteria:\n${acText(item)}\nDo not commit.`,
    { ...IMPL, phase: 'CI fix', label: 'implement:CI', schema: IMPL_S })
  if (!impl || impl.status === 'blocked') return { stage: 'ci-fix', stopped: 'implementer blocked', impl }
  const g = await gateUntilGreen('CI fix', item)
  if (!g) return { stage: 'ci-fix', stopped: 'gate not green after retries', impl }
  log('Breaker skipped for CI fix: test-only change, no product logic.')
  let verdict = null
  for (let r = 1; r <= MAX_ROUNDS; r++) {
    verdict = await agent(`${COMMON}\nAdversarially verify a test-only fix. Criteria:\n${acText(item)}\nInspect git diff (uncommitted, against ${base.sha}). Re-run the TMPDIR=/tmp command yourself, and prove the new premise assertion can fail (e.g. temporarily shorten the nesting, observe red, restore, touch the file, confirm green; leave no probe in the tree). Gate evidence:\n${g.test_summary}\nReport each criterion met, unmet or unverified with evidence; unverified is not met. pass = all met.`,
      { ...SKEPTIC, phase: 'CI fix', label: `skeptic:CI:${r}`, schema: VERDICT_S })
    if (verdict && verdict.pass) break
    const f = await fix('CI fix', item, `Skeptic found unmet criteria:\n${JSON.stringify(unmet(verdict))}`, `fix:CI:${r}`)
    if (!f || f.status === 'blocked' || !(await gateUntilGreen('CI fix', item))) break
  }
  if (!verdict || !verdict.pass) return { stage: 'ci-fix', stopped: 'skeptic did not pass', verdict }
  const c = await commit('CI fix', 'commit:CI',
    'test(cli): breaker3 builds over-cap fallback paths on every OS\n\nThe blank-name fallback test assumed the temp root path is wider than\n40 columns. That holds for macOS /var/folders but not Linux /tmp, so\nthe last cell aligned to the column stop and the test failed in CI.\nThe repos now sit under a nested directory and the test asserts the\nover-cap premise first.')
  return { stage: 'ci-fix', base: base.sha, commit: c, gate: g.test_summary, verdict,
    next: 'Main loop: push impl/v0, confirm both test jobs green, then open PR impl/v0 -> main (do not merge).' }
}

// ---- stage: spec -------------------------------------------------------------
async function spec() {
  phase('Spec')
  const base = await head('Spec', 'base')
  if (!base || !base.clean) return { stage: 'spec', stopped: 'working tree not clean', status: base && base.status }
  const gix = await agent(`${COMMON}\nInvestigation only; edit nothing in the repo. Question for decision 1 (dot-files under .git/refs): with the gix version in Cargo.lock, does opening a repo and iterating/looking up refs succeed, skip, or error when refs/heads/.DS_Store (a 6 KiB regular file of binary junk) exists? Build a probe in a scratch directory outside the repo (a tiny cargo project depending on the same gix version, or an existing repo test helper run against a temp repo), and also check git's own behaviour (git for-each-ref, git status). Quote commands and output. Return a short factual report: gix behaviour, git behaviour, and the code path in crates/rollcall-git that does the pre-open refs/ check (file:line).`,
    { ...IMPL, phase: 'Spec', label: 'investigate:gix-dotfiles' })
  let draft = await agent(`${COMMON}\nYou are the planner. Write spec revision 16 into ${SPEC} and fixed acceptance criteria into ${AC_FILE} (new file) for every item the author approved in HANDOFF.md: I1 refs dot-files, I2 exit code 4, I3 config ignore, I4 tap rename to ravi1395/homebrew-rollcall (dist-workspace.toml + regenerated release.yml), I5a HTML entity decode, I5b tty truncation + colour + --color flag, I5c risk summary line, I5d filters --dirty/--stale/--top, I5e missing-worktree prune hint.\nRules: specify exactly what the author decided, nothing more; where HANDOFF marks an assumption, write it as an assumption the author must confirm and list it in open_questions. Match the spec's existing style (revision notes inline, Status line gains a Revision 16 entry, keep "Awaiting author review"). Update §3.1 (flags, exit codes), §3.2 (ignore key), §4.4 (refs rule, with the gix finding below), §8.2 (plain output), and any test-matrix section the changes affect. Do not touch unrelated text.\nACCEPTANCE.md: one "## <ID> <title>" section per item with "Spec sections:", "Has logic: yes|no", "Files (hint):", then numbered criteria "<ID>-<n>: ..." that are observable and testable (command + expected output or test assertion). Each criterion must be able to fail.\ngix investigation report:\n${gix}\nDo not commit. Return files changed, item ids written, open questions for the author, and a summary.`,
    { model: 'opus', phase: 'Spec', label: 'draft:spec-rev16', schema: SPEC_S })
  if (!draft) return { stage: 'spec', stopped: 'spec author died' }
  let review = null
  for (let r = 1; r <= 2; r++) {
    review = await agent(`${COMMON}\nReview the uncommitted spec revision 16 (git diff ${SPEC}) and ${AC_FILE} against HANDOFF.md's author decisions and design answers. Criteria:\n- S1: every approved item and design answer is specified, with no change of meaning.\n- S2: nothing beyond the author's decisions is specified unless marked as an assumption for the author.\n- S3: every acceptance criterion is observable, testable and able to fail.\n- S4: no text outside the affected sections changed; no stale cross-references (e.g. exit-code lists in other sections, the old tap name).\nReport each as met, unmet or unverified with evidence; pass = all met.`,
      { ...SKEPTIC, phase: 'Spec', label: `review:spec:${r}`, schema: VERDICT_S })
    if (review && review.pass) break
    draft = await agent(`${COMMON}\nRevise the uncommitted spec revision 16 and ${AC_FILE} to fix exactly these review findings, nothing else:\n${JSON.stringify(unmet(review))}\nDo not commit.`,
      { model: 'opus', phase: 'Spec', label: `revise:spec:${r}`, schema: SPEC_S }) || draft
  }
  const c = await commit('Spec', 'commit:spec',
    'docs(spec): revision 16 draft for the 2026-10-03 author decisions\n\nRefs dot-files, exit code 4, config ignore, plain status UX batch,\nprune hint and the tap rename, with fixed acceptance criteria in\ndocs/superpowers/plans/tasks/v1/ACCEPTANCE.md. Awaiting author review.')
  return { stage: 'spec', commit: c, review, open_questions: draft.open_questions, summary: draft.summary,
    next: 'STOP: author reviews spec rev 16 and ACCEPTANCE.md. Run stage "implement" with specApproved: true only after approval.' }
}

// ---- stage: implement ----------------------------------------------------------
async function runItem(item) {
  const id = item.id
  const base = await head('Implement', `base:${id}`)
  if (!base || !base.clean) return { id, status: 'stopped', reason: 'working tree not clean', detail: base && base.status }

  const impl = await agent(`${COMMON}\nImplement item ${id}: ${item.title}. Spec sections: ${item.spec_sections || 'see ' + AC_FILE}. Files (hint): ${(item.files_hint || []).join(', ')}.\nAcceptance criteria (fixed):\n${acText(item)}\nTests first (R3): write tests for each criterion, run them, quote the red output, then implement until green. Ponytail ladder (R4): reuse what the repo has (std::io::IsTerminal, ratatui's crossterm re-export, existing sanitise/text helpers) before writing new code. R1: no Cargo.toml, Cargo.lock, mod list or pinned-signature edits; report "blocked" with the exact change you need instead, after restoring your own edits. Verify with scoped clippy/fmt/tests for the crates you touched and quote "test result:" lines. Do not commit. Return every file you changed.`,
    { ...IMPL, phase: 'Implement', label: `implement:${id}`, schema: IMPL_S })
  if (!impl || impl.status === 'blocked') return { id, status: 'blocked', impl }
  let g = await gateUntilGreen('Implement', item)
  if (!g) return { id, status: 'stopped', reason: 'gate not green after implement', impl }
  const c1 = await commit('Implement', `commit:${id}`, `feat: ${item.title} (spec rev 16, ${id})`)
  if (!c1) return { id, status: 'stopped', reason: 'commit agent died after implement' }

  let brk = null
  const ambiguous = []
  if (item.has_logic) {
    brk = await agent(`Repo: ${REPO}, branch impl/v0. Break item ${id} (${item.title}), committed as ${c1.sha} on top of ${base.sha}. Spec: ${SPEC} (rev 16), sections ${item.spec_sections}. Acceptance criteria are your spec:\n${acText(item)}\nWrite failing tests only (in crates/*/tests/ or #[cfg(test)] modules); never edit product code; never print prompt text from real agent logs. Classify each finding BROKEN (contradicts the criteria/spec) or AMBIGUOUS (spec unclear). Return the files you wrote.`,
      { ...BREAKER, phase: 'Break', label: `breaker:${id}`, schema: BREAK_S })
    if (brk) {
      ambiguous.push(...brk.ambiguous.map(a => ({ id, ...a })))
      await agent(`${COMMON}\nGuard after a breaker run. Compare git status and git diff against ${c1.sha} with the breaker's reported files ${JSON.stringify(brk.files_written)}. Keep test code only: new or changed files under crates/*/tests/, and hunks inside #[cfg(test)] modules. Revert every other change (git checkout -- <path> for whole files; for mixed files restore the non-test hunks only). Return reverted and kept paths.`,
        { ...CHEAP, phase: 'Break', label: `guard:${id}`, schema: GUARD_S })
      if (brk.broken.length) {
        for (let r = 1; r <= MAX_ROUNDS; r++) {
          const f = await fix('Break', item, `Breaker tests that must pass (regression tests; fix the code, never the assertions):\n${JSON.stringify(brk.broken)}`, `breakfix:${id}:${r}`)
          if (!f || f.status === 'blocked') return { id, status: 'blocked', reason: 'breaker fix blocked', breaker: brk, fix: f }
          g = await gateUntilGreen('Break', item)
          if (g) break
        }
        if (!g) return { id, status: 'stopped', reason: 'gate not green after breaker fixes', breaker: brk }
      }
      await commit('Break', `commit:breaker:${id}`, `test: breaker regression tests for ${id}${brk.broken.length ? ' and fixes' : ''}`)
    }
  } else {
    log(`Breaker skipped for ${id}: no executable logic (has_logic=false in ${AC_FILE}).`)
  }

  let verdict = null
  for (let r = 1; r <= MAX_ROUNDS; r++) {
    verdict = await agent(`Repo: ${REPO}, branch impl/v0. Adversarially review item ${id} (${item.title}): changes are git diff ${base.sha}..HEAD (plus any uncommitted fixes). Original acceptance criteria (fixed):\n${acText(item)}\nBreaker report: ${brk ? brk.summary + ' BROKEN: ' + JSON.stringify(brk.broken) : 'skipped (no logic)'}\nGate evidence: ${g ? g.test_summary : 'none'}\nRe-run what you need yourself; treat claims as unverified until you see evidence; mutation-probe at least one criterion's test to prove it can fail (restore and touch the file afterwards; leave no probe). Report each criterion met, unmet or unverified; unverified is not met. Put optional improvements in optional, not in criteria. pass = all met.`,
      { ...SKEPTIC, phase: 'Review', label: `skeptic:${id}:${r}`, schema: VERDICT_S })
    if (verdict && verdict.pass) break
    const f = await fix('Review', item, `Skeptic found these criteria not met:\n${JSON.stringify(unmet(verdict))}`, `reviewfix:${id}:${r}`)
    if (!f || f.status === 'blocked') return { id, status: 'blocked', reason: 'review fix blocked', verdict, fix: f }
    g = await gateUntilGreen('Review', item)
    if (!g) return { id, status: 'stopped', reason: 'gate not green after review fix', verdict }
    await commit('Review', `commit:reviewfix:${id}:${r}`, `fix: ${id} review round ${r}`)
  }
  if (!verdict || !verdict.pass) return { id, status: 'stopped', reason: 'skeptic did not pass after retries', verdict }
  return { id, status: 'accepted', base: base.sha, verdict, ambiguous }
}

async function implement() {
  if (args.specApproved !== true) throw new Error('implement stage needs args.specApproved = true (author must approve spec rev 16 and ACCEPTANCE.md first)')
  phase('Criteria')
  const ac = await agent(`${COMMON}\nRead ${AC_FILE} and return it as structured items, copying every criterion text verbatim (do not paraphrase). Edit nothing. If the file is missing, return items: [].`,
    { ...CHEAP, phase: 'Criteria', label: 'load:criteria', schema: AC_S })
  if (!ac || !ac.items.length) return { stage: 'implement', stopped: `no acceptance criteria in ${AC_FILE}` }
  const wanted = Array.isArray(args.items) ? args.items : ORDER
  const byId = Object.fromEntries(ac.items.map(i => [i.id, i]))
  const queue = ORDER.filter(id => wanted.includes(id) && byId[id]).map(id => byId[id])
  const missing = wanted.filter(id => !byId[id])
  if (missing.length) log(`Not in ${AC_FILE}, skipped: ${missing.join(', ')}`)
  log(`Serial order (items share cli.rs/output.rs/config.rs, so no parallel lanes): ${queue.map(i => i.id).join(' -> ')}`)

  const start = await head('Criteria', 'base:range')
  if (!start || !start.clean) return { stage: 'implement', stopped: 'working tree not clean', status: start && start.status }
  const results = []
  for (const item of queue) {
    const r = await runItem(item)
    results.push(r)
    log(`${item.id}: ${r.status}${r.reason ? ' (' + r.reason + ')' : ''}`)
    if (r.status !== 'accepted') { log('Stopping: later items build on a clean, accepted tree.'); break }
  }

  phase('Final')
  const done = results.filter(r => r.status === 'accepted').map(r => r.id)
  let final = null
  if (done.length) {
    final = await agent(`Repo: ${REPO}, branch impl/v0. Final review of ${start.sha}..HEAD covering items ${done.join(', ')}. Check every criterion of those items in ${AC_FILE} still holds on the integrated tree (later items can break earlier ones), run the full gate (${GATE}) and quote its test result lines, and confirm no Cargo.toml/Cargo.lock/mod-list edits (R1). Report each criterion met, unmet or unverified; pass = all met.`,
      { ...SKEPTIC, phase: 'Final', label: 'skeptic:final', schema: VERDICT_S })
  }
  return { stage: 'implement', range: start.sha, results, final,
    ambiguous_for_author: results.flatMap(r => r.ambiguous || []),
    next: 'Main loop: audit git log/status, push impl/v0, update HANDOFF.md; author answers AMBIGUOUS items.' }
}

// ---- dispatch ----------------------------------------------------------------
const stage = args && args.stage
if (stage === 'ci-fix') return await ciFix()
if (stage === 'spec') return await spec()
if (stage === 'implement') return await implement()
throw new Error('args.stage must be "ci-fix", "spec" or "implement"')
