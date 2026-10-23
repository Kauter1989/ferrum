# Code quality metrics

Three automated gates keep the code base healthy. All run in CI on every
pull request: test coverage, the complexity budget and dependency
licences.

## Test coverage

Measured with [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov),
which uses the compiler's source-based instrumentation
(`-C instrument-coverage`). Every test level counts: unit, property-based,
data layer, GPU parity on lavapipe and UI tests.

```bash
cargo install cargo-llvm-cov            # once
rustup component add llvm-tools-preview # once
make coverage                           # HTML report: target/llvm-cov/html/index.html
```

- The CI job `coverage` uploads `lcov.info` and the HTML report as the
  `coverage` artifact and prints a summary on the run page.
- It fails when line coverage drops below `COVERAGE_FLOOR` in the
  `Makefile` (87 %). The baseline when the gate was introduced was 88.5 %
  of lines and 87.8 % of functions. Raise the floor as coverage improves;
  never lower it to get a pull request through.
- The coverage badge and the *At a glance* table in the README are
  updated by hand; refresh them when the figure changes noticeably.
- Excluded from the figure: the binary entry point (`main.rs`), examples
  and benchmarks.
- WGSL shaders are not instrumented. They are verified by the GPU-parity
  tests against the CPU reference ray caster instead.

The least covered files at the baseline were the interactive parts of the
UI (`tf_editor.rs` 58 %, `slice_view.rs` 61 %, `app.rs` 68 %). They are the
first candidates for new kittest tests.

## Complexity budget

Enforced by clippy as part of `clippy -D warnings`, with thresholds in
`clippy.toml`:

| Lint | Threshold | Measures |
|---|---|---|
| `cognitive_complexity` | 25 | how hard a function is to follow: branches, loops and their nesting |
| `too_many_lines` | 120 | function length |
| `excessive_nesting` | 6 | depth of nested blocks |

`make complexity` runs only these three lints. Cognitive complexity is
used instead of cyclomatic complexity because it also penalises nesting,
which is what makes code hard to read. For a one-off report of the
classic metrics (cyclomatic, Halstead, maintainability index), use
Mozilla's [`rust-code-analysis-cli`](https://github.com/mozilla/rust-code-analysis).

When a function exceeds the budget, split it into named steps rather than
adding `#[allow(...)]`; an `allow` needs a comment explaining why the
function cannot be split.

## Dependency licences

FERRUM is licensed under MIT OR Apache-2.0 and is meant to be embedded in
commercial and research products, so every dependency must carry a
permissive licence. [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny)
checks the whole dependency graph against the allow-list in `deny.toml`:

```bash
cargo install cargo-deny --locked  # once
make licenses                      # cargo deny check licenses
```

The CI job `licenses` fails when a dependency brings a licence outside the
list. Adding a licence to `deny.toml` is a deliberate decision: copyleft
licences (GPL, AGPL, LGPL without an alternative) are not accepted.

## Python lint and format

The bridges, scripts and Claude Code hooks are checked with
[ruff](https://docs.astral.sh/ruff/), using the rules in `ruff.toml`:
pycodestyle, pyflakes, import order, bugbear, pyupgrade, bandit (`S`,
security), simplifications and ruff's own rules.

The gate is a ratchet. The repository had about 240 findings when it was
introduced, so ruff checks the **Python files a branch changes**, not
the whole tree. Older files are brought in line when they are next
touched. It runs:
- after every edit of a `.py` file (Claude Code hook F), which formats
  the file, applies safe fixes and reports what is left;
- before every push (hook E) on the files changed since
  `origin/develop`;
- in the CI job `python-lint` on the files a pull request changes,
  together with the hook tests.

```bash
pipx install ruff==0.16.9      # once; CI uses the same version
ruff check --fix <files> && ruff format <files>
```

`S101` (assert) is allowed in tests. `S603`/`S607` (subprocess by name)
are allowed in `.claude/`, whose hooks run git, cargo and ruff on
purpose.

## Review agents

Before a PR is pushed, the read-only subagents `code-reviewer` and
`security-reviewer` (`.claude/agents/`) review the branch
(`ferrum-change` §6). Their blocking findings are fixed or answered in
the PR body. They add judgement on top of the automated gates and do not
replace them.
