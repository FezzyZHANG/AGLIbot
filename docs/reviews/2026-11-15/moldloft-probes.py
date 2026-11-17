"""Isolated read-only-source audit probes; writes only disposable SQLite fixtures under target."""
import json
import os
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
os.environ.pop("TICKETS_ENV_FILE", None)
os.environ["TICKETS_COPILOT"] = "off"
os.environ["TICKETS_TRANSLATION"] = "off"
sys.path.insert(0, str(ROOT / "Moldloft" / "src"))

from tickets.app import create_app
from starlette.requests import Request
from starlette.responses import Response
from tickets.db import initialize
from tickets.notes import Notebook
from tickets.service import Store
from tickets.proposals import Proposals, coherent_graph
from tickets import basis
from tickets.review import render

def emit(name, result):
    print(json.dumps({"probe": name, **result}, ensure_ascii=False))

# The real middleware coroutine has no suspended await on an unscoped route.
# This tests the guard, not an end-to-end browser or network attack.
app = create_app(ROOT / "target/audit-20261115/unused.db",
                 notes_path=ROOT / "target/audit-20261115/unused-notes",
                 repo_roots={}, translator=None)
guard = next(m.kwargs["dispatch"] for m in app.user_middleware
             if m.kwargs.get("dispatch").__name__ == "local_boundary")
async def downstream(request):
    return Response("guard allowed downstream", status_code=204)
for origin in ["http://attacker.example:8787", "http://different.example:8787"]:
    request = Request({"type": "http", "method": "POST", "scheme": "http",
        "path": "/probe", "raw_path": b"/probe", "root_path": "", "query_string": b"",
        "server": ("127.0.0.1", 8787), "client": ("127.0.0.1", 1),
        "headers": [(b"host", b"attacker.example:8787"), (b"origin", origin.encode())]})
    coroutine = guard(request, downstream)
    try:
        coroutine.send(None)
    except StopIteration as finished:
        emit("HTTP Host/Origin guard", {"origin": origin, "status": finished.value.status_code})

# Real SQLite reducers and proposal persistence, with only the filesystem rendering sink
# replaced by an in-memory call to the production renderer.
with tempfile.TemporaryDirectory(prefix="moldloft-review-probe-", dir=Path(__file__).parent) as temp:
    fixture = Path(temp)
    engine = initialize(fixture / "tickets.db")
    try:
        book = Notebook(fixture / "notes", engine)
        store = Store(engine, notes=book)
        proposals = Proposals(store)
        renderings = []
        proposals.files.write = lambda proposal, sections: renderings.append(render(proposal, sections))
        store.mutate([{"op": "create", "fields": {"title": "Before"}}])
        captured = basis.capture(coherent_graph(store), [1])
        pending = proposals.submit([{"op": "update", "ticket_id": 1, "fields": {"title": "After"}}],
            captured, "Synthetic update", "Check retained review evidence.", "free", "audit-review-before-after")
        before = renderings[-1]
        proposals.apply(pending["id"])
        after = renderings[-1]
        emit("Review diff retention", {
            "before_apply_has_old_title": '- title: "Before"' in before,
            "before_apply_has_new_title": '+ title: "After"' in before,
            "after_apply_has_old_title": '- title: "Before"' in after,
            "after_apply_has_new_title": '+ title: "After"' in after,
            "final_title": store.graph()["tickets"][0]["title"]})
        captured = basis.capture(coherent_graph(store), [1])
        adoption = proposals.submit([{"op": "note_create", "temp_id": "new:premise",
            "path": "notes/n-test-adopted.md", "fields": {"title": "Synthetic premise",
            "kind": "assumption", "state": "adopted", "body": "Only a test"}}],
            captured, "Synthetic adopted proposal", "Check documented proposal restriction.",
            "free", "audit-adopted-proposal")
        emit("Documented adoption restriction", {
            "agent_proposal_adopted_state": adoption["operations"][0]["fields"]["state"],
            "status": adoption["status"], "note_written": book.read("n-test-adopted") is not None})
    finally:
        engine.dispose()
