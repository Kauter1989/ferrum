# Skill evaluations

The evaluations are tasks with known answers on synthetic phantoms. They
check that an agent using this skill:
- gets the right number;
- states the unit;
- took its numbers from tool results rather than from images;
- makes no diagnostic claims;
- asks for review when it created proposals;
- used the commands a task requires (e.g. `segment interactive`).

```bash
ferrum-cli eval tasks                         # ids and prompts
ferrum-cli eval phantoms /tmp/ferrum-evals    # writes sphere_cube.nii.gz
ferrum-cli eval engine 127.0.0.1:8765         # tasks with "engine": true — FERRUM's mock engine
# run your agent on a task's prompt with the phantom, record a transcript:
#   { "calls": [ { "command": "probe", "arguments": {…}, "result": {…envelope…} } ], "answer": "…" }
ferrum-cli eval grade --task sphere_diameter transcript.json   # exit 0 = passed
```

`tasks.json` lists the tasks. FERRUM's test suite contains a reference
solution for each task (`crates/ferrum-agent/tests/evals.rs`), which
proves that the answers can be reached with the tools.
