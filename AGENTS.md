<!-- moldloft:project:begin -->
## Moldloft project entry

Moldloft source is in `Moldloft/`. Read [the Moldloft README](Moldloft/README.md) for application usage.
Run `python "Moldloft/tools/project.py" check --project .` to inspect the installation;
run `python "Moldloft/tools/project.py" run --project .` to start the local application.
Run these commands from this project root, or pass its absolute path.
Before changing Moldloft source, read [its development contract](Moldloft/AGENTS.md).
Use the application API for live ticket edits; installation checks never open the database.
<!-- moldloft:project:end -->

## Who may write core code

Core code is `src/` and anything at L3 or above in [DESIGN section 6](docs/DESIGN.md#6-自进化与安全).
L4 (`config/`, boundaries, `evolution/gate.rs`) is owner-only regardless of model.

- **Direct writers:** Astra (`gpt-6-astra`, via Codex) and Opus (via the owner's Claude Code).
- **Directed writer:** `deepseek-v4.1-flash-1017`, only for well-defined tasks. A task is
  well-defined when an advanced model (Astra or Opus) directs it: the director writes the
  plan, names the files and interfaces to touch and the tests that decide acceptance, and
  reviews the resulting diff before it reaches the owner. The directed model does not widen
  scope; on ambiguity it stops and hands back to the director instead of guessing.
- **Everyone else** (other models, and AGLI's own chat/judge models) may read and propose,
  never write core code.

Claude Code is the owner's tool; AGLI never invokes the `claude` CLI. Core-code work that AGLI
initiates enters as a Moldloft ticket or proposal, and the owner decides who implements it.
AGLI's delegated hands write only inside `work/tasks/<id>/`. Rationale:
`workspace/moldloft/notes/n-agli-dec-hands.md`.
