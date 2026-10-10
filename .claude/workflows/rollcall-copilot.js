export const meta = {
  name: 'rollcall-copilot',
  description: 'rollcall Copilot CLI session source: Opus drafts spec rev 17 + ACCEPTANCE.md (stops for author), then serial per-phase implement (Haiku xhigh or Sonnet high by complexity) + Opus breaker/skeptic',
  whenToUse: 'rollcall impl/copilot after the 2026-10-07 handoff. args.stage = "spec" | "spec-review" (needs args.since = commit before the draft) | "implement" (needs args.specApproved = true; optional args.items = ["P1", ...])',
  phases: [
    { title: 'Preflight', detail: 'Branch, clean tree, author answers Q1-Q7 present in HANDOFF.md' },
    { title: 'Spec', detail: 'Opus drafts spec rev 17 + ACCEPTANCE.md, Opus skeptic review; stops for author', model: 'opus' },
    { title: 'Criteria', detail: 'Load fixed acceptance criteria from ACCEPTANCE.md' },
    { title: 'Implement', detail: 'Per phase, serial: implement (P1 Haiku xhigh; P2-P4 Sonnet high), gate, commit' },
    { title: 'Break', detail: 'Per phase: Opus breaker, guard, tiered fixes', model: 'opus' },
    { title: 'Review', detail: 'Per phase: Opus skeptic, tiered fix loop', model: 'opus' },
    { title: 'Final', detail: 'Opus skeptic over the whole range' },
  ],
}

// ---- constants -------------------------------------------------------------
const REPO = '/Users/ravichandrasekhar/Projects/rollcall'
const BRANCH = 'impl/copilot'
const SPEC = 'docs/superpowers/specs/2026-09-29-rollcall-design.md'
const PLAN = 'docs/superpowers/plans/2026-10-07-rollcall-copilot-plan.md'
const RESEARCH = 'docs/research/2026-10-07-copilot-cli-sessions.md'
const PROBE = 'HANDOFF.md step 1b ("Probe run" and "Probe re-run", session 15: real Copilot CLI 1.0.93 key sets)'
const AC_FILE = 'docs/superpowers/plans/tasks/copilot/ACCEPTANCE.md'
const RULES = 'docs/superpowers/plans/tasks/v0/WORKER-RULES.md'
const ANSWERS = 'Author answers 2026-10-07 (Copilot plan)'
const GATE = 'cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked'
const TRAILER = 'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'
const MAX_ROUNDS = 3
// Implementer per phase, by complexity (author, 2026-10-07): Haiku xhigh for mechanical work,
// Sonnet high for parser/wiring logic. P1 is enum arms + schema enums (plan tags them mech);
// P2 is the events.jsonl parser and incremental reads; P3 is cross-file wiring plus the test
// sandbox (a miss reads the real ~/.copilot); P4 is a resolve-time TextRef and a hand-rolled reader.
const HAIKU = { model: 'haiku', effort: 'xhigh' }
const SONNET = { model: 'sonnet', effort: 'high' }
const TIER = { P1: HAIKU, P2: SONNET, P3: SONNET, P4: SONNET }
// A Haiku phase whose first fix attempt did not hold escalates to Sonnet high.
const implOpts = (id, round = 1) => (TIER[id] === HAIKU && round >= 2 ? SONNET : TIER[id] || SONNET)
const CHEAP = { model: 'sonnet', effort: 'low' }
const BREAKER = { agentType: 'breaker', model: 'opus' }
const SKEPTIC = { agentType: 'skeptical-reviewer', model: 'opus' }
// P5 (real-data check) reads the author's real ~/.copilot, so it runs in the main loop, never here.
const ORDER = ['P1', 'P2', 'P3', 'P4']
// Approved R1 exceptions, by plan phase. Anything else structural is a stop-and-report.
const R1_ALLOWED = {
  P1: 'the SourceKind::Copilot variant in crates/rollcall-core/src/lib.rs, and the one line `pub mod copilot;` in crates/rollcall-sessions/src/lib.rs',
  P3: 'the field `copilot_home` in EnvPaths (crates/rollcall-cli/src/config.rs)',
  P4: 'only what HANDOFF.md section "' + ANSWERS + '" approves for Phase 4 (e.g. a new TextRef variant if Q1=(b))',
}

const COMMON = `Repo: ${REPO}, branch ${BRANCH}, main checkout (no worktree). Read HANDOFF.md section "${ANSWERS}", the plan ${PLAN}, the research ${RESEARCH}, and the real-data probe results in ${PROBE} first. Where the probe and the research disagree, the probe wins. Repo CLAUDE.md rules R1-R4 bind you; ${RULES} applies except its worktree and target-dir lines (you work in the main checkout with its own target/). Other Claude sessions may share this branch: never revert, stage or commit changes you did not make. Never push. Never read or print anything under the real ~/.copilot, ~/.config/github-copilot, ~/.claude or ~/.codex (prompt text and tokens live there); tests use sandboxed COPILOT_HOME/HOME and hand-written fixtures only. No Cargo.toml or Cargo.lock edit, ever. Use Bash timeouts of 600000 ms for cargo.`

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
const HEAD_S = { type: 'object', properties: { sha: STR, branch: STR, clean: { type: 'boolean' }, status: STR },
  required: ['sha', 'branch', 'clean', 'status'] }
const ANSWERS_S = { type: 'object', properties: {
  found: { type: 'boolean' },
  answers: { type: 'array', items: { type: 'object', properties: { q: STR, answer: STR }, required: ['q', 'answer'] } },
  r1_approved: { type: 'boolean' }, missing: STRS },
  required: ['found', 'answers', 'r1_approved', 'missing'] }
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
const r1Line = id => `R1: allowed structural edits for ${id}: ${R1_ALLOWED[id] || 'none'}. Any other mod-list or pinned-signature change, or any Cargo.toml/Cargo.lock change: restore your own edits and report "blocked" with the exact change you need.`

const head = (ph, label) => agent(`${COMMON}\nRun git rev-parse HEAD, git branch --show-current and git status --short -- ':!.claude' ':!graphify-out' (the pathspecs exclude the untracked local .claude/ dir and graphify-out/, which graphify update regenerates). Edit nothing. Return sha, branch, clean (true iff that status output is empty) and the status text verbatim.`,
  { ...CHEAP, phase: ph, label, schema: HEAD_S })

const gate = (ph, label) => agent(`${COMMON}\nRun the full gate from the repo root and edit nothing:\n${GATE}\nReturn pass (true only if every command exited 0), test_summary (every "test result:" line verbatim, plus the total passed/failed/ignored you add up), and failures (names of failing tests or the first error lines of fmt/clippy).`,
  { ...CHEAP, phase: ph, label, schema: GATE_S })

// The tree is verified clean (outside .claude/) at the start of every stage and item, and work is serial,
// so every changed path belongs to this step: commit them all, by explicit path.
const commit = (ph, label, message) => agent(`${COMMON}\nThe author started this workflow and authorized its local commits on ${BRANCH}; this commit step is part of that request, so make it (never push). Run git status --short -- ':!.claude' ':!graphify-out'. Stage every listed path (modified, added, deleted, untracked) by explicit path; never git add -A, git add . or anything under .claude/ or graphify-out/. If nothing is listed, return the current HEAD sha and note "nothing to commit". The commit message is the text between the two --- lines, verbatim, followed by a blank line and the trailer line:\n---\n${message}\n---\n${TRAILER}\nAfter committing run graphify update . if graphify is on PATH (ignore if absent). Return the new sha and the committed paths.`,
  { ...CHEAP, phase: ph, label, schema: COMMIT_S })

async function preflight(ph) {
  const base = await head(ph, 'base')
  if (!base) return { stopped: 'head agent died' }
  if (base.branch !== BRANCH) return { stopped: `on branch "${base.branch}", need ${BRANCH} (git switch -c ${BRANCH} origin/main, then cherry-pick the plan/handoff commits)` }
  if (!base.clean) return { stopped: 'working tree not clean', status: base.status }
  const ans = await agent(`${COMMON}\nRead HANDOFF.md. Find the section headed "${ANSWERS}". Return found (whether it exists), answers (one entry per open question Q1-Q7 of ${PLAN}, the author's answer quoted verbatim; omit a question with no answer), r1_approved (true only if that section explicitly approves the plan's R1 items: SourceKind::Copilot, pub mod copilot, EnvPaths.copilot_home), and missing (ids of Q1-Q7 with no answer). Edit nothing.`,
    { ...CHEAP, phase: ph, label: 'answers', schema: ANSWERS_S })
  if (!ans || !ans.found || ans.missing.length || !ans.r1_approved)
    return { stopped: `author answers incomplete in HANDOFF.md "${ANSWERS}"`, answers: ans }
  return { base, answers: ans }
}

async function fix(ph, item, problem, label, round = 1) {
  return agent(`${COMMON}\nYou fix plan phase ${item.id} (${item.title}). Acceptance criteria (fixed; do not reinterpret):\n${acText(item)}\nProblem to fix:\n${problem}\nFix product code to satisfy the criteria. Never weaken, delete or edit the assertions of an existing test, including breaker tests, unless a criterion above names that exact assertion as changing; if a test itself contradicts the spec, stop and report status "blocked" with the contradiction. ${r1Line(item.id)} Run the scoped tests that cover your change and quote their "test result:" lines. Do not commit.`,
    { ...implOpts(item.id, round), phase: ph, label, schema: IMPL_S })
}

async function gateUntilGreen(ph, item) {
  for (let r = 1; r <= MAX_ROUNDS; r++) {
    const g = await gate(ph, `gate:${item.id}:${r}`)
    if (g && g.pass) return g
    const f = await fix(ph, item, `The full gate failed:\n${g ? g.failures.join('\n') : 'gate agent died'}\n${g ? g.test_summary : ''}`, `gatefix:${item.id}:${r}`, r)
    if (!f || f.status === 'blocked') return null
  }
  return null
}

// Review -> revise until the skeptic passes; the loop always ends on a review, so the text
// that gets committed is the text the last verdict judged.
const SPEC_ROUNDS = 3
async function reviewSpec(since, draft, reviewLabel, reviseLabel) {
  let review = null
  for (let r = 1; r <= SPEC_ROUNDS; r++) {
    review = await agent(`${COMMON}\nReview spec revision 17: git diff ${since} -- ${SPEC} CLAUDE.md (working tree against ${since}, so committed and uncommitted changes both count), and the whole of ${AC_FILE} (it may be untracked) against ${PLAN} Phase 0, the research ${RESEARCH} and HANDOFF.md "${ANSWERS}". Criteria:\n- S1: every author answer is specified with no change of meaning; nothing beyond plan + answers + the probe results in ${PROBE} is specified unless marked *assumption*.\n- S2: §5.3 states root, files, id, cwd, branch, started/ended, human prompt, title, last_prompt, message_count, children, unknown types and incremental rules, and each matches the probe results (field names, event names, pendingGitContext pair, resume after shutdown), and the research where the probe is silent (ephemeral title).\n- S3: every acceptance criterion is observable, testable and able to fail; P5 is absent; each item names its R1 allowance.\n- S4: no text outside the affected sections changed; no stale cross-references (source enums listing only claude/codex, env lists without COPILOT_HOME).\n- S5: no real prompt text or token appears anywhere in the diff.\n- S6: each probe correction (dot-dir skip uncounted, parentAgentTaskId not a sub-agent marker, model.* and session.usage_checkpoint known, resume after shutdown, zero-prompt session) is specified and has a P2 criterion; only facts the probe did not confirm are marked *assumption*.\nReport each as met, unmet or unverified with evidence; pass = all met.`,
      { ...SKEPTIC, phase: 'Spec', label: `${reviewLabel}:${r}`, schema: VERDICT_S })
    if ((review && review.pass) || r === SPEC_ROUNDS) break
    draft = await agent(`${COMMON}\nRevise spec revision 17 (${SPEC}, ${AC_FILE} and the CLAUDE.md Safety line, as they stand in the working tree; changes since ${since}) to fix exactly these review findings, nothing else:\n${JSON.stringify(unmet(review))}\nDo not commit.`,
      { model: 'opus', phase: 'Spec', label: `${reviseLabel}:${r}`, schema: SPEC_S }) || draft
  }
  return { review, draft }
}

// ---- stage: spec-review (re-review a committed spec rev 17 draft) ----------------
async function specReview() {
  if (!/^[0-9a-f]{7,40}$/.test(String(args.since || ''))) throw new Error('spec-review needs args.since = the commit before the spec rev 17 draft')
  phase('Preflight')
  const pre = await preflight('Preflight')
  if (pre.stopped) return { stage: 'spec-review', ...pre }
  phase('Spec')
  const { review, draft } = await reviewSpec(args.since, null, 'rereview:spec', 'rerevise:spec')
  const c = await commit('Spec', 'commit:spec-review',
    'docs(spec): revision 17 review fixes, Copilot CLI session source\n\nSkeptic findings on the rev 17 draft fixed in the spec and\ndocs/superpowers/plans/tasks/copilot/ACCEPTANCE.md. Awaiting author review.')
  const after = await head('Spec', 'verify:commit:spec-review')
  if (!after || !after.clean) return { stage: 'spec-review', stopped: 'commit step left the tree dirty', status: after && after.status, commit: c, review }
  return { stage: 'spec-review', since: args.since, commit: c, review_passed: !!(review && review.pass), review,
    open_questions: draft ? draft.open_questions : [], summary: draft ? draft.summary : 'no revision needed',
    next: review && review.pass
      ? 'STOP: author reviews spec rev 17 and ACCEPTANCE.md. Run stage "implement" with specApproved: true only after approval.'
      : 'STOP: the skeptic still fails rev 17 after ' + SPEC_ROUNDS + ' rounds; the main loop reads the verdict and decides.' }
}

// ---- stage: spec (plan Phase 0) ------------------------------------------------
async function spec() {
  phase('Preflight')
  const pre = await preflight('Preflight')
  if (pre.stopped) return { stage: 'spec', ...pre }
  phase('Spec')
  const answers = pre.answers.answers.map(a => `${a.q}: ${a.answer}`).join('\n')
  let draft = await agent(`${COMMON}\nYou are the planner. Execute Phase 0 of ${PLAN}: write spec revision 17 into ${SPEC} and fixed acceptance criteria into ${AC_FILE} (new file), and add ~/.copilot to the Safety line of the repo CLAUDE.md.\nAuthor answers (binding):\n${answers}\nSpec: add §5.3 Copilot CLI (root, files, id, cwd, branch, started/ended, human prompt, title, last_prompt, message_count, children, unknown types, incremental rules) using the research's worked-record skeletons (shapes only, never prompt text); the source enum (§3.4/§7) and schema_version per Q7; resume in §8.1; COPILOT_HOME in §10.1; a §4.4 audit note only if a new probe is approved. Fold in the probe corrections from ${PROBE}: discovery skips dot-named entries of session-state/ (e.g. .session-operation-locks/) without counting them as non_uuid; parentAgentTaskId is not a sub-agent marker (Q4 excludes on the envelope agentId only); every type the probe saw (model.*, session.usage_checkpoint, session.model_change, session.shutdown, session.info, session.error, system.message, assistant.*) is a known type; session.resume appends to the same log after a session.shutdown, so one log may hold several shutdowns; specify a session with no human user.message the same way §5.0 treats one for Claude/Codex. Facts the probe confirmed are not *assumption*; mark as *assumption* only what neither the probe nor a docs/source-rated research fact confirms (at least agentId, isAutopilotContinuation, rewind truncation). The Status line (line 3) gains a "Revision 17" entry first and keeps "Awaiting author review". Do not touch unrelated text.\nACCEPTANCE.md: copy the Conventions block of docs/superpowers/plans/tasks/v1/ACCEPTANCE.md and add COPILOT_HOME to its env list; then one "## <ID> <title>" section per plan phase to implement (P1, P2, P3, and P4 only if the answers enable it; never P5), each with "Spec sections:", "Has logic: yes|no", "Files (hint):", "R1 allowed:", then numbered criteria "<ID>-<n>: ..." taken from the plan's phase criteria, resolved per the author's answers (drop the per-option text that no longer applies). Each criterion must be observable and able to fail. P2 must include one criterion per probe correction: a dot-named dir is skipped and not counted as non_uuid; a human user.message carrying parentAgentTaskId is counted; model.* and session.usage_checkpoint lines leave unknown_type at 0; start, shutdown, resume, then a /cd pair gives the post-/cd cwd and branch; the zero-prompt session behaves as specified.\nDo not commit. Return files changed, item ids written, any remaining open questions, and a summary.`,
    { model: 'opus', phase: 'Spec', label: 'draft:spec-rev17', schema: SPEC_S })
  if (!draft) return { stage: 'spec', stopped: 'spec author died' }
  const out = await reviewSpec(pre.base.sha, draft, 'review:spec', 'revise:spec')
  draft = out.draft
  const review = out.review
  const c = await commit('Spec', 'commit:spec',
    'docs(spec): revision 17 draft, Copilot CLI session source\n\nNew §5.3 for ~/.copilot/session-state/<id>/events.jsonl, the copilot\nsource kind and COPILOT_HOME, per the 2026-10-07 plan and author\nanswers, with fixed acceptance criteria in\ndocs/superpowers/plans/tasks/copilot/ACCEPTANCE.md. Awaiting author review.')
  const after = await head('Spec', 'verify:commit:spec')
  if (!after || !after.clean) return { stage: 'spec', stopped: 'commit step left the tree dirty', status: after && after.status, commit: c, review }
  return { stage: 'spec', commit: c, review_passed: !!(review && review.pass), review, open_questions: draft.open_questions, summary: draft.summary,
    next: review && review.pass
      ? 'STOP: author reviews spec rev 17 and ACCEPTANCE.md. Run stage "implement" with specApproved: true only after approval.'
      : `STOP: the skeptic did not pass the final text. Fix the record, then run stage "spec-review" with since: "${pre.base.sha}".` }
}

// ---- stage: implement (plan Phases 1-4) ----------------------------------------
async function runItem(item) {
  const id = item.id
  const base = await head('Implement', `base:${id}`)
  if (!base || !base.clean) return { id, status: 'stopped', reason: 'working tree not clean', detail: base && base.status }

  const impl = await agent(`${COMMON}\nImplement plan phase ${id}: ${item.title}. Spec sections: ${item.spec_sections || 'see ' + AC_FILE}. Files (hint): ${(item.files_hint || []).join(', ')}.\nAcceptance criteria (fixed):\n${acText(item)}\nTests first (R3): write tests for each criterion, run them, quote the red output, then implement until green. Fixtures are hand-written JSONL in the probe's confirmed key sets (${PROBE}; the research's shapes where the probe is silent) with a marker prompt string such as PROMPT-MARKER, never copied from a real store. Ponytail ladder (R4): reuse crates/rollcall-sessions/src/jsonl.rs (open_log, plan_read, head_fp/tail_fp, resolve_log_ref, file_exists_same) and the claude.rs patterns before writing new code. ${r1Line(id)} Verify with scoped clippy/fmt/tests for the crates you touched and quote "test result:" lines. Do not commit. Return every file you changed.`,
    { ...implOpts(id), phase: 'Implement', label: `implement:${id}`, schema: IMPL_S })
  if (!impl || impl.status === 'blocked') return { id, status: 'blocked', impl }
  let g = await gateUntilGreen('Implement', item)
  if (!g) return { id, status: 'stopped', reason: 'gate not green after implement', impl }
  const c1 = await commit('Implement', `commit:${id}`, `feat(copilot): ${item.title} (spec rev 17, ${id})`)
  if (!c1) return { id, status: 'stopped', reason: 'commit agent died after implement' }

  let brk = null
  const ambiguous = []
  if (item.has_logic) {
    brk = await agent(`Repo: ${REPO}, branch ${BRANCH}. Break plan phase ${id} (${item.title}), committed as ${c1.sha} on top of ${base.sha}. Spec: ${SPEC} (rev 17), sections ${item.spec_sections}; real-data probe results (they win over the research): ${PROBE}; research: ${RESEARCH}. Acceptance criteria are your spec:\n${acText(item)}\nAttack the parser and wiring: torn/partial lines, truncation (rewind), pending vs settled context_changed, resume after shutdown, agentId events, a human user.message carrying parentAgentTaskId, dot-named dirs in session-state/, zero-prompt sessions, symlinked session dirs or logs, FIFOs, non-UUID dir names, oversize lines, prompt-text leaks into notes or diagnostics, COPILOT_HOME empty/unset, resolve after the file changed. Write failing tests only (in crates/*/tests/ or #[cfg(test)] modules); never edit product code; never read the real ~/.copilot or print prompt text from any real agent log. Classify each finding BROKEN (contradicts the criteria/spec) or AMBIGUOUS (spec unclear). Return the files you wrote.`,
      { ...BREAKER, phase: 'Break', label: `breaker:${id}`, schema: BREAK_S })
    if (brk) {
      ambiguous.push(...brk.ambiguous.map(a => ({ id, ...a })))
      await agent(`${COMMON}\nGuard after a breaker run. Compare git status and git diff against ${c1.sha} with the breaker's reported files ${JSON.stringify(brk.files_written)}. Keep test code only: new or changed files under crates/*/tests/, and hunks inside #[cfg(test)] modules. Revert every other change (git checkout -- <path> for whole files; for mixed files restore the non-test hunks only). Return reverted and kept paths.`,
        { ...CHEAP, phase: 'Break', label: `guard:${id}`, schema: GUARD_S })
      if (brk.broken.length) {
        for (let r = 1; r <= MAX_ROUNDS; r++) {
          const f = await fix('Break', item, `Breaker tests that must pass (regression tests; fix the code, never the assertions):\n${JSON.stringify(brk.broken)}`, `breakfix:${id}:${r}`, r)
          if (!f || f.status === 'blocked') return { id, status: 'blocked', reason: 'breaker fix blocked', breaker: brk, fix: f }
          g = await gateUntilGreen('Break', item)
          if (g) break
        }
        if (!g) return { id, status: 'stopped', reason: 'gate not green after breaker fixes', breaker: brk }
      }
      await commit('Break', `commit:breaker:${id}`, `test(copilot): breaker regression tests for ${id}${brk.broken.length ? ' and fixes' : ''}`)
    }
  } else {
    log(`Breaker skipped for ${id}: no executable logic (has_logic=false in ${AC_FILE}).`)
  }

  let verdict = null
  for (let r = 1; r <= MAX_ROUNDS; r++) {
    verdict = await agent(`Repo: ${REPO}, branch ${BRANCH}. Adversarially review plan phase ${id} (${item.title}): changes are git diff ${base.sha}..HEAD (plus any uncommitted fixes). Original acceptance criteria (fixed):\n${acText(item)}\n${r1Line(id)} Check git diff ${base.sha}..HEAD -- Cargo.lock '*Cargo.toml' is empty and that structural edits stay within that allowance.\nBreaker report: ${brk ? brk.summary + ' BROKEN: ' + JSON.stringify(brk.broken) : 'skipped (no logic)'}\nGate evidence: ${g ? g.test_summary : 'none'}\nRe-run what you need yourself; treat claims as unverified until you see evidence; mutation-probe at least one criterion's test to prove it can fail (restore and touch the file afterwards; leave no probe). Never read the real ~/.copilot. Report each criterion met, unmet or unverified; unverified is not met. Put optional improvements in optional, not in criteria. pass = all met.`,
      { ...SKEPTIC, phase: 'Review', label: `skeptic:${id}:${r}`, schema: VERDICT_S })
    if (verdict && verdict.pass) break
    const f = await fix('Review', item, `Skeptic found these criteria not met:\n${JSON.stringify(unmet(verdict))}`, `reviewfix:${id}:${r}`, r)
    if (!f || f.status === 'blocked') return { id, status: 'blocked', reason: 'review fix blocked', verdict, fix: f }
    g = await gateUntilGreen('Review', item)
    if (!g) return { id, status: 'stopped', reason: 'gate not green after review fix', verdict }
    await commit('Review', `commit:reviewfix:${id}:${r}`, `fix(copilot): ${id} review round ${r}`)
  }
  if (!verdict || !verdict.pass) return { id, status: 'stopped', reason: 'skeptic did not pass after retries', verdict }
  return { id, status: 'accepted', base: base.sha, verdict, ambiguous }
}

async function implement() {
  if (args.specApproved !== true) throw new Error('implement stage needs args.specApproved = true (author must approve spec rev 17 and ACCEPTANCE.md first)')
  phase('Preflight')
  const pre = await preflight('Preflight')
  if (pre.stopped) return { stage: 'implement', ...pre }
  phase('Criteria')
  const ac = await agent(`${COMMON}\nRead ${AC_FILE} and return it as structured items, copying every criterion text verbatim (do not paraphrase). Edit nothing. If the file is missing, return items: [].`,
    { ...CHEAP, phase: 'Criteria', label: 'load:criteria', schema: AC_S })
  if (!ac || !ac.items.length) return { stage: 'implement', stopped: `no acceptance criteria in ${AC_FILE}` }
  const wanted = Array.isArray(args.items) ? args.items : ORDER
  const byId = Object.fromEntries(ac.items.map(i => [i.id, i]))
  const queue = ORDER.filter(id => wanted.includes(id) && byId[id]).map(id => byId[id])
  const missing = wanted.filter(id => !byId[id])
  if (missing.length) log(`Not in ${AC_FILE}, skipped: ${missing.join(', ')}`)
  log('P5 (real-data check on the real ~/.copilot) is not run here: the main loop runs it after the author supplies a sample (plan Q6).')
  log(`Serial order (P1 adds the enum every later phase matches on; P3 wires what P2 builds): ${queue.map(i => i.id).join(' -> ')}`)

  const start = pre.base
  const results = []
  for (const item of queue) {
    const r = await runItem(item)
    results.push(r)
    log(`${item.id}: ${r.status}${r.reason ? ' (' + r.reason + ')' : ''}`)
    if (r.status !== 'accepted') { log('Stopping: later phases build on a clean, accepted tree.'); break }
  }

  phase('Final')
  const done = results.filter(r => r.status === 'accepted').map(r => r.id)
  let final = null
  if (done.length) {
    final = await agent(`Repo: ${REPO}, branch ${BRANCH}. Final review of ${start.sha}..HEAD covering plan phases ${done.join(', ')}. Check every criterion of those phases in ${AC_FILE} still holds on the integrated tree (later phases can break earlier ones), run the full gate (${GATE}) and quote its test result lines, confirm git diff ${start.sha}..HEAD -- Cargo.lock '*Cargo.toml' is empty and every mod-list or pinned-signature change is one of: ${JSON.stringify(R1_ALLOWED)}. Never read the real ~/.copilot. Report each criterion met, unmet or unverified; pass = all met.`,
      { ...SKEPTIC, phase: 'Final', label: 'skeptic:final', schema: VERDICT_S })
  }
  return { stage: 'implement', range: start.sha, results, final,
    ambiguous_for_author: results.flatMap(r => r.ambiguous || []),
    next: 'Main loop: audit git log/status, update HANDOFF.md, run P5 if the author supplied a sample, then ask before pushing impl/copilot; author answers AMBIGUOUS items.' }
}

// ---- dispatch ----------------------------------------------------------------
const stage = args && args.stage
if (stage === 'spec') return await spec()
if (stage === 'spec-review') return await specReview()
if (stage === 'implement') return await implement()
throw new Error('args.stage must be "spec", "spec-review" or "implement"')
