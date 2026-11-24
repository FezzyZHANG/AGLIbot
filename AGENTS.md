## Moldloft project entry

Moldloft v2 source is in `Moldloft/`. Read [the Moldloft README](Moldloft/README.md) for application usage.
`ops/moldloft.ps1` runs its CLI on this project's workspace (`workspace/moldloft/data/moldloft.db`,
with this checkout as repository `aglibot`) and installs it into `.tools/moldloft/` on first use.
With no arguments it serves the workbench at http://127.0.0.1:8787, bound to the LAN.
Other arguments pass through: `ops/moldloft.ps1 ready --json`, `ops/moldloft.ps1 ls --notes`,
or `ops/moldloft.ps1 mcp` for an MCP client over stdio.
Make live edits through the CLI, the workbench or MCP proposals; never write the database directly.
`workspace/moldloft/archive/` is a tracked export for reading, not the live store. Don't edit it;
after changing tickets or notes, refresh it with `ops/moldloft.ps1 export workspace/moldloft/archive`.
Before changing Moldloft source, read [its development contract](Moldloft/AGENTS.md).

## Who may write core code

Core code is `src/` and anything at L3 or above in [DESIGN section 6](docs/DESIGN.md#6-自进化与安全).
L4 (`config/`, boundaries) is owner-only regardless of model. The in-process evolution gate
(`evolution/gate.rs`) was retired with the owner's sign-off on 2026-11-23; core code now ships only
through Git and `ops/deploy.ps1`, and bringing a gate back is itself an L4 change the owner starts.

- **Direct writers:** Astra (`gpt-6.2-astra`, via Codex) and Opus (via the owner's Claude Code).
- **Directed writer:** Sol (`gpt-6.2-sol`, via Codex), only for well-defined tasks.
  `deepseek-v4.1-flash-1017` held this role until 2026-11-25 and is parked until the owner
  restores it. A task is well-defined when an advanced model (Astra or Opus) directs it:
  the director writes the plan, names the files and interfaces to touch and the tests that
  decide acceptance, and reviews the resulting diff before it reaches the owner. The directed model does not widen
  scope; on ambiguity it stops and hands back to the director instead of guessing.
- **Everyone else** (other models, and AGLI's own chat/judge models) may read and propose,
  never write core code.

Claude Code is the owner's tool; AGLI never invokes the `claude` CLI. Core-code work that AGLI
initiates enters as a Moldloft ticket or proposal, and the owner decides who implements it.
AGLI's delegated hands write only inside `work/tasks/<id>/`. Rationale:
`workspace/moldloft/archive/graphs/default/notes/n-agli-dec-hands.md`.
