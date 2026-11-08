---
name: ferrum-report
description: Write the self-contained HTML report on a merged or merge-ready feature PR (or a stage of several PRs) — architectural changes, code analysis with design patterns, and a review history with fixed, rejected and deferred findings and an honest quality assessment. Use after the PR merges or when the user asks for a report on it. The report goes beside the feature's design doc in docs/<feature>/; every number is derived from git, never from memory. Not a substitute for the lessons entry — lessons.md is for the next agent, this report is for the person reviewing the feature.
---

# Feature report

A report on **one feature PR**, or a **stage** of several PRs, written after
it is merged or merge-ready. It answers three questions a person who did
not follow the work will ask: what is the architecture now, does the code
hold up, and what did the review actually catch.

The reference implementation is
`docs/webgpu-panel/pr-91-report.html` (on branch `docs/pr-91-report`). Read
it once before writing a new one, and copy its structure and CSS rather
than designing a different page.

## 1. Track and place

- **Track: `small`** (`ferrum-workflow` §1) — this is `docs` work.
  Branch `docs/<feature>-report` from a fresh `origin/develop`
  (`new-branch`), the report in one commit, the commit trail in review
  (`ferrum-change` §7).
- **The file goes beside the feature's design doc:**

  ```
  docs/<feature>/<pr-or-stage>-report.html
  docs/webgpu-panel/pr-91-report.html      # stage 25–26, PR #91, its fix #92
  ```

  Feature name = the topic of the design doc (`docs/webgpu-panel.md` →
  `docs/webgpu-panel/`), PR number first when the report is about a PR.
  The reference lives at exactly that path.
- **No folder yet:** create `docs/<feature>/` and move nothing. The design
  doc stays where it is (`docs/<topic>.md`); the folder holds reports
  only. Do not rename, split or relocate an existing doc as a side effect
  of writing a report.
- Do not touch `docs/architecture.md`, an ADR or `.claude/skills/*` in this
  branch. A change to the system is not part of reporting on it.

## 2. The track is not the lessons entry

Both exist, for different readers, and one does not replace the other:

| Artifact | Reader | Shape |
|---|---|---|
| `.claude/lessons.md` (`ferrum-lessons`) | the next agent | L/W entries: symptom, mechanism, guard |
| `docs/<feature>/*-report.html` (this skill) | the person reviewing the feature | architecture, patterns, review history, grades |

The report *names* the lessons it produced (the reference's stat card
reads "7 lessons recorded (L29–L37)"), and each review finding it
discusses ideally points at the lesson or guard that came out of it. It
never carries the lesson text instead of writing the entry.

## 3. Collect the facts first

The report is written **after** the facts are collected, from commands.
Never from memory, from the PR description's own claims, or from what the
session "remembers" being changed. Record every command's output in a
scratch file as you go; the report is assembled from it.

```bash
# shape of the change
git log --oneline develop..feat/<branch>            # commits, for the count and chronology
git diff --shortstat develop...<merge>              # files, +/-
git diff --stat develop...<merge>                   # where the lines went
git diff --name-status develop...<merge>            # added / changed / deleted modules

# line counts per module, for the "modules added" table
git ls-tree -r --name-only <merge> -- crates/ferrum-view | xargs wc -l

# review history
git log --format='%h %s' --grep='<finding key>'      # the commit a finding led to

# the criteria and what verified them
git show <merge>:dev_plan.md | sed -n '/^## Stage <N>/,/^## /p'
```

Then add the non-git sources:
- the **PR bodies** (verification table, assumptions, review answers);
- the **review agent reports** (`code-reviewer`, `security-reviewer`,
  `ferrum-change` §6) — finding, level, and the answer given;
- the **CI history**, for red rounds.

Rules for the numbers:
- **After the merge, `develop..branch` is empty** — the commits are on
  `develop` now. Diff the merge against its own base instead:
  `git diff --shortstat <merge>^1...<merge>`, or the develop tip from before
  the merge. Run every command once and read its output before trusting the
  pattern; an empty result means the base is wrong, not that nothing changed.
- "55 files, +5 472 / −38" comes from `--shortstat`; "~2 000 lines of new
  code" is stated as an approximation with the command that produced it.
- Count commits, findings, fixed, deferred and open with a command or from
  the report table itself; the stat cards and the tables must agree.
- **A number you cannot derive is not written.** Say "not measured" in
  the footer if a figure was unavailable.

## 4. The page

Structure, in this order (the reference's order — keep it):

```
h1  <PR or stage> — <what the feature does>
p.sub  one line: technical report: architecture, code analysis, review history
div.meta   branch → target · state (merged <date> / merge-ready) · commits · shortstat · one honest pill
div.grid   stat cards: modules added · new lines · findings · fixed before merge · lessons
h2 1. Architectural changes
     h3 Modules added      table: module · role · lines · key
     h3 Modules changed    table: module · what changed
     h3 Links              inline SVG diagram + caption
     h3 Layering           list: who may depend on what, verified how
h2 2. Code analysis and design patterns
     one subsection per pattern that is actually present, with the file and a
     short <pre><code> excerpt
     h3 Where the patterns fit, and where they do not   (fit / debatable / not)
     h3 Tests that cannot fail                          table: mutation → what fails
h2 3. Review stages
     how many rounds, from which source, how many findings, how many fixed, which commit
     h3 Findings in detail         table: level · finding · decision and reason
     h3 Security review            table + "checked as clean" card
     h3 Quality assessment         table: aspect · grade · justification
     h3 Chronology
footer   sources and the commands the numbers came from
```

### Self-contained, offline

- **One file, no requests.** No CDN, no web font, no remote image, no
  external stylesheet or script. It must render from a `file://` URL and
  inside GitHub's HTML viewer, offline.
- **Diagrams are inline `<svg>`.** Classes for nodes, edges, labels and
  zones (`node`, `node.accent`, `node.strong`, `edge`, `edge.dashed`,
  `e-label`, `zone`), an arrow `<marker>`, `currentColor` for strokes, an
  `aria-label` on the `<svg>`, plus a `diagram-caption` under it saying
  what solid vs dashed means. The reference diagram is a good template.
- CSS in one `<style>` block. No `<script>` unless something must be
  interactive; then it is inline kind (which GitHub's viewer drops), and
  the page must still read without it.

### Light and dark

The page will be read in both, so both are defined once, at `:root`, and
nothing hardcodes a colour. The reference's palette:

```css
:root {
  --bg:#fbfbfd; --fg:#1d1f26; --muted:#5b6070; --line:#e2e5ec;
  --accent:#1f6feb; --ok:#1a7f37; --warn:#9a6700; --bad:#b42318;
  --card:#ffffff; --code:#f3f4f7;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg:#12141a; --fg:#e6e7ea; --muted:#a2a8b6; --line:#2a2e38;
    --accent:#6ba6ff; --ok:#4ac26b; --warn:#d4a72c; --bad:#ff7b72;
    --card:#191c23; --code:#1f232c;
  }
}
```

- SVG fills and strokes take their colours from the same variables
  (`fill: var(--code)`, `stroke: currentColor`), never a literal hex.
  A diagram that is legible in one theme and grey on grey in the other is
  the usual failure (L35).
- Tints come from `color-mix(in srgb, var(--ok) 16%, transparent)`, so the
  chip, pill and tag colours follow the theme without a second palette.

### Tables and marks

- Findings and grades are tables, not prose; `td.num` right-aligns with
  tabular figures.
- Every finding carries **its level** and **its outcome** as a tag:

  | Class | Tag | Meaning |
  |---|---|---|
  | `t-block` | `blocking` | a bug, a broken rule, a test that cannot fail |
  | `t-should` | `should-fix`, `low`, `medium` | worth fixing in this PR |
  | `t-nit` | `nit` | mechanical, no discussion needed |
  | `t-fixed` | `fixed` / `fails` | fixed, or the test that catches it |

  Outcome words, used exactly: **Fixed.** **Rejected.** **Deferred.**
  **Open, named.** Each followed by the reason, not just the label.

## 5. Honest by construction

The report is only worth reading if it is trusted. These rules are not
tone choices; a report that breaks one is a defect of the same class as a
test that cannot fail.

- **Rejected findings are reported as rejected, with the reason.**
  "Rejected. In the code `dims/2`; for 512×512×252 that is
  `[256, 256, 126]` — the middle. Checked by arithmetic, not by reading.
  False finding." A rejected finding is not deleted, and not softened
  into "addressed".
- **Deferred is deferred.** Name what was deliberately not done and why
  it did not belong in this PR (an ADR missing, a protocol change, a
  look-in-one-place fix that would hide risk at the end of an XL PR), and
  say who or what will do it.
- **Open work is listed as open**, in the findings table and in the
  quality assessment. Nothing is left open silently.
- **The quality assessment names the weak aspect as weak.** The reference
  grades *Process* "weak" — skipped reviews, a missing size gate, a red
  suite treated as background — while grading *Architecture* and
  *Documentation* "strong". A row that grades everything strong is a
  review that did not happen. Each grade has its justification in the
  same row, including the one bad point ("one debatable spot — the CPU
  overlay").
- **State what was verified and what was not**, the same discipline as
  the verification table in `ferrum-verify`.
- **Errors in the process are part of the record.** Run review late, or
  get a finding wrong, and the report says so — "the review agents were
  run only after the PR had been pushed many times, on the user's direct
  question" is the most useful paragraph in the reference.
- No patient identifiers, no volume data, no machine-specific paths
  (`ferrum-lessons`, *Keep identifiers and data out*). Paths inside the
  repository are fine.

## 6. Verify before delivering

Open the page in a browser and look at it. Then check:

| Check | How |
|---|---|
| Renders | open the `file://` URL; the page is not blank, the `<style>` took effect |
| Sections | all three `h2` are present, in order, and no placeholder text is left |
| Tables | every table has its header row and cells; no `\|` or `<` leaked as text |
| Diagram | the SVG is drawn — boxes, arrowheads, labels — not an empty frame |
| No horizontal overflow | the page does not scroll sideways at ~1280 px and ~1000 px |
| No requests | everything inline; the network panel stays empty on load |
| Both themes | light and dark (emulate `prefers-color-scheme: dark`); diagram legible in both |
| No console errors | the console is empty |

State in the commit message **which check was made** — for example
"rendered the page from `file://` in Chrome at 1280 px and 1000 px, light
and dark: three sections, all tables, the diagram drawn, no horizontal
scroll, no console errors". A report claimed as verified but not opened is
a false claim (L33's class).

Commit message: `docs: a report on PR #<n> — architecture, code analysis,
review history`, with the attribution lines the session requires
(`ferrum-change` §7). Branch and PR as in §1; merging waits for the user
(G3).

## 7. When not to write one

- The feature is not merged and not merge-ready: the numbers and the
  review history are still moving. Wait, or write the report once the
  last review round is answered.
- The work is a single obvious fix: a lesson entry or the PR body is the
  right size. This skill is for a feature PR or a stage.