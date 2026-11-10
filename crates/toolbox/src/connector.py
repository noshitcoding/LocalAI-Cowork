"""Pinned MCP connector. URL and bearer arrive only through encrypted environment slots.

Discovery never contacts the destination. Each send_json executes one POST with no
redirects or retries. Response bodies/headers are deliberately not exposed: webhook
services may echo the secret path or Authorization header.
"""
import ipaddress
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

LIMIT = 256 * 1024


class DeliveryUncertain(Exception):
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def configuration():
    value = json.loads(os.environ["CONNECTOR_CONFIG"])
    url = urllib.parse.urlsplit(value["url"])
    if url.username or url.password or url.fragment or not url.hostname:
        raise ValueError("Invalid connector destination")
    loopback = False
    try:
        loopback = ipaddress.ip_address(url.hostname).is_loopback
    except ValueError:
        pass
    if url.scheme != "https" and not (url.scheme == "http" and loopback):
        raise ValueError("Connector requires HTTPS or a literal loopback address")
    if any(ord(c) < 32 or ord(c) == 127 for c in value["url"] + value["bearer"]):
        raise ValueError("Invalid connector credentials")
    return value


def definition(destination, label):
    return {"name": "send_json", "title": label + " — " + destination,
            "description": "Send one JSON POST to " + destination + ". Endpoint: " + label +
            ". The configured path and credentials stay private. This action can modify external data; approve the concrete payload.",
            "inputSchema": {"type": "object", "properties": {
                "destination": {"type": "string", "const": destination},
                "payload": {"type": "object"}},
                "required": ["destination", "payload"], "additionalProperties": False},
            "_meta": {"io.localai.connector": True},
            "annotations": {"readOnlyHint": False, "destructiveHint": True,
                            "idempotentHint": False, "openWorldHint": True}}


def send_json(config, destination, arguments):
    if set(arguments) != {"destination", "payload"} or arguments["destination"] != destination or not isinstance(arguments["payload"], dict):
        raise ValueError("Connector destination and payload must match the reviewed tool")
    raw = json.dumps(arguments["payload"], ensure_ascii=False, allow_nan=False).encode("utf-8")
    if len(raw) > LIMIT:
        raise ValueError("Connector payload exceeds 256 KiB")
    headers = {"Content-Type": "application/json", "Accept": "application/json"}
    if config["bearer"]:
        headers["Authorization"] = "Bearer " + config["bearer"]
    request = urllib.request.Request(config["url"], data=raw, headers=headers, method="POST")
    # Explicit empty ProxyHandler avoids sending a webhook credential to an ambient proxy.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    try:
        response = opener.open(request, timeout=30)
    except urllib.error.HTTPError as error:
        response = error
    except Exception:
        raise DeliveryUncertain() from None
    with response:
        status = response.status
        # No response content, headers, Location or reason is returned or logged.
    return {"content": [{"type": "text", "text": json.dumps({"destination": destination, "status": status})}],
            "isError": not 200 <= status < 300}


def main():
    config = configuration()
    destination, label = sys.argv[1:3]
    # Rust provides the same normalized public origin in this pinned catalog.
    if urllib.parse.urlsplit(config["url"]).netloc != urllib.parse.urlsplit(destination).netloc:
        raise ValueError("Connector destination does not match its frozen catalog")
    for line in sys.stdin.buffer:
        if len(line) > 2 * 1024 * 1024:
            return
        request = json.loads(line)
        if "id" not in request:
            continue
        try:
            method = request.get("method")
            if method == "initialize":
                result = {"protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                          "serverInfo": {"name": "cowork-connector", "version": "1"}}
            elif method == "tools/list":
                result = {"tools": [definition(destination, label)]}
            elif method == "ping":
                result = {}
            elif method == "tools/call" and request.get("params", {}).get("name") == "send_json":
                result = send_json(config, destination, request["params"]["arguments"])
            else:
                raise ValueError("Unsupported connector method")
            response = {"jsonrpc": "2.0", "id": request["id"], "result": result}
        except DeliveryUncertain:
            # Drop the transport without a reply, preserving MCP's uncertain-effect
            # checkpoint. Reconnection must never replay this tools/call.
            return
        except Exception:
            # Never stringify network errors: they can contain the secret URL.
            response = {"jsonrpc": "2.0", "id": request["id"], "error": {
                "code": -32000, "message": "Connector call failed; delivery may be uncertain. Do not resend without reviewing the external service."}}
        sys.stdout.buffer.write(json.dumps(response, ensure_ascii=False).encode("utf-8") + b"\n")
        sys.stdout.buffer.flush()


if __name__ == "__main__":
    main()
