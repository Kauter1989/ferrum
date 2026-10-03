---
name: ferrum-field-test
description: Prepare and evaluate a field test of a FERRUM branch on the user's own machine (GPU, WSL, real engines, real data) — the only place where engines, VRAM, real studies and drivers are exercised. Use when a change touches engines/bridges, rendering, input formats or performance, when the user asks for a test script, or when the user sends back results (steps.txt, benchmark.md, logs, PNGs). Encodes the failures of earlier test scripts.
---

# Field tests on the user's machine

The cloud session has no GPU, no engine weights and no real studies. The
user's machine has all three. A field test there found the outline bug
(L1), the missing NIfTI modality (L10) and the threshold leak (L14), and
gave the GPU table (W1). It also cost runs through script mistakes (L11,
L12). These rules prevent that.

## 1. Ask first

Before writing the script, ask the user for what you would otherwise
guess (L13):
- OS and shell (Windows + WSL Ubuntu, native Linux);
- the exact GPU model and VRAM (`nvidia-smi`);
- RAM and free disk space;
- network restrictions (proxy, DNS);
- whether they want their own study or a public dataset.

## 2. Write the script by these rules

The script is attached to the chat, not committed (the user's rule). It
lives in the scratchpad, and you send it as a file.

1. **Preflight that stops early, with a clear message (L12):**
   - name resolution and HTTPS to github.com, pypi.org and
     download.pytorch.org;
   - `nvidia-smi`;
   - free disk and RAM (TotalSegmentator `total` needs about 16 GB of
     RAM, so in WSL set `memory=` in `.wslconfig`).
2. **Always build the branch under test (L11):**
   - `git fetch origin <branch>`, then `git checkout -B <branch>
     origin/<branch>`, then `cargo build --release -p ferrum-cli`;
   - log `git log --oneline -1` and `ferrum-cli --version`;
   - the "skip setup" option skips only the heavy engine and Python
     installs, never this step.
3. **PyTorch:** install `torch` and `torchvision` together from one
   index (for example `cu128`) in one command, then verify with
   `import torchvision.ops; torchvision.ops.nms`. A mismatch shows up as
   `operator torchvision::nms does not exist` (L12).
4. **Optional tools degrade, they do not abort.** Without `nvidia-smi`,
   memory is "not measured" (`scripts/benchmark_engines.py` already does
   this).
5. **Every step leaves a trace.** Use one `run` helper that:
   - writes the JSON envelope to a numbered file;
   - appends a line to `steps.txt`: name, time, `ok`, warnings,
     `error.code`/`message`/`hint`;
   - when the command printed **no JSON**, puts the tail of its stderr
     into `steps.txt` (L11):
   ```bash
   run() {
     local name="$1"; shift; STEP=$((STEP + 1))
     local file; file=$(printf '%s/%02d-%s.json' "$OUT" "$STEP" "$name")
     local t0; t0=$(date +%s.%N)
     "$CLI" "$@" >"$file" 2>>"$LOGS/cli-stderr.log" || true
     local ok; ok=$(jq -r '.ok' "$file" 2>/dev/null || echo false)
     printf '%-34s %6.1f s  ok=%s\n' "$name" "$(awk -v a="$t0" -v b="$(date +%s.%N)" 'BEGIN{print b-a}')" "$ok" | tee -a "$OUT/steps.txt"
     if [[ ! -s "$file" ]]; then
       echo "    no JSON output — ferrum-cli said:" | tee -a "$OUT/steps.txt"
       tail -n 5 "$LOGS/cli-stderr.log" | sed 's/^/    /' | tee -a "$OUT/steps.txt"
     fi
     jq -r '.warnings[]? | "    ! " + .' "$file" 2>/dev/null | tee -a "$OUT/steps.txt"
     [[ "$ok" == true ]] || jq -r '"    error: " + .error.code + ": " + .error.message' "$file" 2>/dev/null | tee -a "$OUT/steps.txt"
     LAST="$file"
   }
   ```
6. **Data without patient identifiers:**
   - prefer a public, labelled, de-identified set (MSD Task09 Spleen,
     CC BY-SA 4.0) so Dice against ground truth is possible;
   - never ask for a real patient's study to be sent back;
   - NIfTI needs `study open --modality CT` (L10).
7. **The script expects FERRUM's guard rails.** A `limit` from
   `segment threshold` is a designed outcome (L14): try a narrower range
   once, then continue with the next step.
8. **Renders for a person to look at:**
   - axial slices through the segmented organs, at 768 px, in `outline`
     and `fill_outline`;
   - the user sends them back (skill `ferrum-visual-check`, step 5).
9. **Measure:** run `scripts/benchmark_engines.py`. It records time and
   GPU memory per step and writes `benchmark.md`.
10. **Check the syntax.** Run `bash -n` before sending. Tell the user it
    has not run here.

## 3. Tell the user what to send back

- `steps.txt` and `benchmark.md`;
- `logs/` (bridge logs, `cli-stderr.log`, `gpu.txt`);
- the PNGs of step 8.

## 4. Read the results

1. **Separate script failures from FERRUM failures (W5).** For each
   failed step, reproduce it locally with the current branch and a
   synthetic study (see `phantom_render.py` for a NIfTI writer).
   - It passes here: suspect the environment or the script (stale
     binary, versions).
   - It fails here: it is a FERRUM bug.
2. **Compare with ground truth:** Dice, volume, HD95. Compare with the
   design's assumptions too, and correct the docs where they disagree
   (L13).
3. **Look at every PNG.** Most visual bugs are found here (L1).
4. **Work out each finding:**
   - a FERRUM bug: a regression test, a fix, and a lesson entry;
   - a script bug: a corrected script and a lesson entry;
   - a doc correction: a commit in the feature branch.
5. **Report to the user:** what works, what failed and why, what you
   changed.

## 5. Record

After each field test, add the findings with skill `ferrum-lessons`,
including what the test caught that CI could not.
