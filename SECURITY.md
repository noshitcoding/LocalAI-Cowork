# Security

Report vulnerabilities privately through
[GitHub Security Advisories](https://github.com/noshitcoding/LocalAI-Cowork/security/advisories/new).
Do not include real credentials or personal files in public issues. Use
synthetic examples and redact diagnostics.

Include the exact component/version, platform, selected local/server connection,
crossed security boundary, reproduction and expected versus observed behavior.

The root app is V2; enterprise/ is independently versioned and operated. Preview
status and source tests are not production acceptance. Models, MCP servers,
terminals, document engines and runtime backends have their own permissions and
data routes. Review the endpoint, execution plan and approval before use.

Release bytes require source-bound checksums, dependency inventories and an
accurate signing/update status. The preview workflow requires an independent
trusted signing/producer policy, disabled until real values are configured.
