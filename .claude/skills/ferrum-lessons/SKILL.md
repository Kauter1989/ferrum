---
name: ferrum-lessons
description: Record a lesson in FERRUM's development memory (.claude/lessons.md) — a bug, a wasted push or CI round, a broken script, a wrong assumption, or an approach that clearly worked. Use right after such a finding, at the end of a task that had one, when the user says "remember this" / "запомни", and before starting work to recall relevant lessons.
---

# The lessons log

`.claude/lessons.md` is the project's memory across sessions. Every
other skill (`ferrum-change`, `ferrum-visual-check`, `ferrum-field-test`,
`ferrum-release`) reads it and points to its entries. It works only if
entries are honest, specific and come with a guard.

## When to add an entry

**A failure (`L<n>`):**
- a bug found after it was committed, by a test, a review, CI, a field
  test or the user;
- a push, CI round, or a run on the user's machine wasted by a mistake;
- an assumption in a design or doc that turned out wrong;
- a rule the user had to correct you on (for example, pushing to
  `develop`).

**What worked (`W<n>`):** an approach that found a bug or saved a round
and should be repeated.

Do not add routine work, or bugs that a test caught before the commit
unless the bug class is worth remembering (L4).

## Format

Append to the right section, with the next free number. Never renumber
or rewrite old entries: if one is superseded, add a new one that says
so.

```markdown
### L<n> — <what went wrong, in a few words> (<YYYY-MM>, <stage or version>)

- **Symptom:** what was observed, and by whom.
- **Root cause:** the actual mechanism, with the file and function.
- **Why it was missed:** which test or check could not see it.
- **Fix:** what changed (commit or PR if known).
- **Guard:** the test name and file, the skill step, or the rule that
  catches it next time. Required.
- **Rule:** one or two sentences to apply to future work.
```

```markdown
### W<n> — <what worked> (<YYYY-MM>, <stage or version>)

- **What:** the approach.
- **Outcome:** what it found or saved, with numbers if any.
- **Keep:** when to use it again.
```

## Writing a good entry

- **Name the mechanism, not the mood.** "`on_edge` compared only the
  right and lower neighbour" is useful. "Outline code was buggy" is not.
- **Say why the existing tests passed.** That sentence usually tells you
  which guard to add.
- **The guard must exist when the entry is written.** Write the test
  first, or add the step to the relevant skill.
  - If the lesson changes how work is done, edit that skill's step and
    cite the entry (for example, "(L11)").
  - If it is a hard rule for the whole repo, add it to `CLAUDE.md`
    too.
- **Keep identifiers and data out.** No patient data and no
  machine-specific paths beyond what is needed.

## Retro (phase 10 of `ferrum-workflow`)

After a stage, or a PR that had trouble, go through these questions:
- What did the field test, a reviewer or CI find that our tests did
  not? Each answer is an L entry with a guard.
- Which skill step was missing, wrong or skipped? Fix the skill and cite
  the entry.
- Did the size estimate hold? Record a calibration row
  (`ferrum-size` §5).
- What worked well enough to repeat? Each answer is a W entry.

## Committing

The entry goes in the same branch as the fix and is reviewed with it. If
there is no code change (for example, a lesson from a field-test
script), use a `docs/` branch and a PR into `develop`. It is never
pushed directly to `develop`.
