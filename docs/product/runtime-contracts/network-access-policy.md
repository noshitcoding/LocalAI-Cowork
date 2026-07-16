---
title: Autonomous Network Access Policy Contract
type: api
doc_type: compatibility
status: current
owner: product-docs
last_updated: 2026-07-09
last_verified: 2026-07-09
endpoint: Tauri IPC web_fetch_url and web_search; crew pipeline web_fetch and web_search
purpose: Prevent autonomous web tools from reaching local or private services and bound all downloaded text.
userStory: As a user I can enable web tools without exposing loopback, LAN, cloud metadata, or binary download surfaces to an agent.
visibleText: none
sizeToken: none
states: allowed, policy-denied, sandbox-denied, network-blocked, redirected, truncated, http-error
interactions: fetch public URL, search the web, follow validated redirect, stop at byte limit
dataSource: public HTTP and HTTPS endpoints resolved through the operating-system DNS resolver
accessibility: not applicable
tests: network_safety.rs tests, lib.rs policy tests, registry.ts web tool tests
source_files:
  - app/src-tauri/src/network_safety.rs
  - app/src-tauri/src/lib.rs
  - app/src/engine/tools/registry.ts
canonical_for:
  - autonomous HTTP SSRF protection
  - web response type and size limits
  - redacted web-tool audit metadata
rationale: Model-generated URLs are untrusted input. Frontend validation and automatic HTTP redirect handling are not security boundaries.
---

# Autonomous Network Access Policy Contract

Autonomous web fetches accept only absolute `http` and `https` URLs on ports 80 and 443. URL credentials, local hostnames, private or special-use IPv4 and IPv6 ranges, loopback, link-local, carrier-grade NAT, multicast, cloud metadata addresses, and IPv4-mapped bypasses are rejected before a request is sent.

Every domain lookup must return at least one address, and every returned address must be public. The validated addresses are pinned into the request client so a second DNS lookup cannot redirect the connection to a private target. System proxies are disabled for this autonomous path. Provider endpoints are a separate, explicitly configured trust boundary and may intentionally use localhost; this contract does not apply to provider health or inference calls.

Redirects are handled manually. Each target is parsed, resolved, and authorized again; loops, more than five redirects, local/private destinations, unsafe ports, unsupported schemes, and HTTPS-to-HTTP downgrades are blocked. MCP servers and raw user-controlled terminal traffic have separate capability contracts and are not implicitly authorized by this policy.

Responses must declare a textual Content-Type. Text, JSON, XML, RSS, and Atom payloads are accepted; binary and missing content types are rejected except for HTTP 204. Streaming stops after 2 MiB, and the user-facing character limit is applied afterward. Redirect and request processing share a 30-second budget.

Audit events record the run, active policy, outcome, stage, status, result counts, and URL origin only. URL paths, queries, fragments, and search text are not written; search events retain only the query length. Policy, sandbox, SSRF, content-type, and HTTP failures are audited as well as successful calls.
