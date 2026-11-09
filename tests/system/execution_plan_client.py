"""Current owner protocol used by disposable system fixtures, without IO replay."""
from __future__ import annotations

import json

from urllib.parse import quote


def reviewed_run(call, request):
    """Prepare the original request and confirm only its exact retained fingerprint.

    Fixture callers explicitly authorize their own starts. There is no transport
    retry, new request key, provider fallback or outcome inference in this helper.
    """
    prepared = call("POST", "/execution-plans", request)
    plan = prepared["plan"]
    assert plan["order_id"] == request["order_id"]
    assert plan["idempotency_key"] == request["idempotency_key"]
    assert not prepared["retired"]
    path = "/orders/{}/execution-plans/{}/commit".format(
        quote(plan["order_id"], safe=""), quote(plan["id"], safe="")
    )
    return call("POST", path, {"sha256": prepared["sha256"]})


def selected_folder_capabilities(messages):
    """Read the declared fixture grants from split or merged system messages.

    OpenAI-compatible providers merge system instructions on the wire. Only
    system messages may supply these fixture capabilities; user text cannot.
    """
    marker = "Selected folder capabilities (use these IDs; no other folders are granted): "
    selected = []
    for message in messages:
        content = message.get("content")
        if message.get("role") == "system" and isinstance(content, str) and marker in content:
            grants, _ = json.JSONDecoder().raw_decode(content.split(marker, 1)[1].lstrip())
            assert isinstance(grants, list), "Expected a folder capability list"
            selected.append(grants)
    assert len(selected) == 1, "Expected exactly one system folder capability declaration"
    return selected[0]
