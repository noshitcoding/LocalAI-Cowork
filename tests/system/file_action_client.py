"""Explicit, snapshot-bound file-action review for disposable system fixtures.

Callers retain the returned review for every explicit recheck/commit. This helper
never retries IO, generates a replacement key or recaptures inputs on replay.
"""
from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
from urllib.parse import quote


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def endpoint(kind):
    assert kind in {"import", "restore"}, "Expected an import or restore fixture"
    return "/execution-plans/v2/file-" + ("imports" if kind == "import" else "restorations")


@dataclass(frozen=True)
class FileActionReview:
    kind: str
    wire_request: str
    wire_plan: str
    sha256: str

    @property
    def request(self):
        return json.loads(self.wire_request)

    @property
    def plan(self):
        return json.loads(self.wire_plan)


def checked_review(kind, wrapped, prepared):
    plan = prepared["plan"]
    assert not prepared["retired"], "Original file action was retired"
    assert plan["order_id"] == wrapped["request"]["order_id"]
    assert plan["idempotency_key"] == wrapped["request"]["idempotency_key"]
    assert plan["input"]["id"] == wrapped["input"]["snapshot_id"]
    assert plan["workspace"]["input"] == wrapped["input"]
    assert plan["preflight"]["kind"] == kind
    # Match the TS client: hash the server's exact canonical UTF-8 bytes, then
    # bind their decoded object to the separately returned plan. Re-encoding
    # JSON would introduce a second floating-point serialization convention.
    assert json.loads(prepared["canonical_json"]) == plan
    assert hashlib.sha256(prepared["canonical_json"].encode()).hexdigest() == prepared["sha256"]
    return FileActionReview(kind, canonical(wrapped), canonical(plan), prepared["sha256"])


def prepare_file_action(call, kind, request):
    """Capture once and prepare; retain the returned immutable review object."""
    endpoint(kind)
    original = json.loads(canonical(request))
    order = original["order_id"]
    selection = {
        "idempotency_key": original["idempotency_key"] + "-input",
        "base_revision": original["base_revision"],
        "folder_ids": [original[kind]["folder_id"]],
    }
    captured = call("POST", "/orders/{}/execution-inputs".format(quote(order, safe="")), selection)
    assert not captured["retired"] and captured["snapshot"]["order_id"] == order
    assert captured["snapshot"]["order_revision"] == original["base_revision"]
    assert captured["sha256"] == hashlib.sha256(canonical(captured["snapshot"]).encode()).hexdigest()
    wrapped = {
        "target": {"kind": "broker_controller"},
        "request": original,
        "input": {
            "snapshot_id": captured["snapshot"]["id"],
            "sha256": captured["sha256"],
            "manifest_sha256": captured["snapshot"]["manifest"]["sha256"],
        },
    }
    return checked_review(kind, wrapped, call("POST", endpoint(kind), wrapped))


def recheck_file_action(call, review):
    """Explicitly recheck the original wrapper; never acquire another input."""
    checked = checked_review(review.kind, review.request,
                             call("POST", endpoint(review.kind), review.request))
    assert checked == review, "Original file-action plan or fingerprint changed"
    return checked


def commit_file_action(call, review):
    """Confirm exactly the caller-retained plan ID and original fingerprint."""
    plan = review.plan
    path = "/orders/{}/execution-plans/v2/{}/commit".format(
        quote(plan["order_id"], safe=""), quote(plan["id"], safe="")
    )
    return call("POST", path, {"sha256": review.sha256})
