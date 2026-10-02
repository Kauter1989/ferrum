# FERRUM agent skill package

`ferrum/` is the skill:
- `SKILL.md` tells an agent when and how to use FERRUM safely;
- `reference/` holds the pages it reads when needed;
- `schemas/commands.json` holds the parameter schemas (generated with
  `ferrum-cli schema`);
- `evals/` holds tasks with known answers and the grader instructions.

**Installation in a harness:**

| Harness | How |
|---|---|
| MCP-capable (any) | Run `ferrum-cli mcp` as a stdio server. Give the agent `SKILL.md` as instructions or as a skill. |
| Skill folders | Copy `ferrum/` into the harness's skill directory. Put `ferrum-cli` on the `PATH`. |
| Claude Code plugin | The release archives contain `ferrum-plugin/`: the skill, a plugin manifest (`.claude-plugin/plugin.json`), an MCP configuration that starts the bundled `bin/ferrum-cli mcp`, and the binary. |

The operator configures what the agent may read and output in
`ferrum-agent.toml` (`FERRUM_AGENT_CONFIG`). See FERRUM's
`docs/agent-cli.md`.

`plugin/` holds the manifest templates that the release workflow fills
in (`__EXE__` becomes `.exe` on Windows).
