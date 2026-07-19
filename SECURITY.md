# Security Policy

LocalAI Cowork can access local files, terminals, model providers, MCP servers, and other tools selected by the user. Please report security problems privately so a fix can be prepared before public disclosure.

## Supported Versions

Security fixes target the latest published release and the current `main` branch. Older releases may not receive backports.

## Vulnerability Exceptions

Target-specific exceptions are published as OpenVEX documents under `.vex/` and consumed by Trivy in CI. An exception must name one advisory and package, explain why the vulnerable code is outside the maintained execution path, and be revisited before the supported platform set changes. VEX statements do not disable secret, dependency, Semgrep, or filesystem scanning.

## Report A Vulnerability

Use [GitHub private vulnerability reporting](https://github.com/noshitcoding/LocalAI-Cowork/security/advisories/new).

Please include:

- The affected version and Windows version
- The security boundary that was crossed
- Minimal, reproducible steps or a proof of concept
- The expected and observed behavior
- The potential impact
- Any suggested mitigation

Do not include real API keys, tokens, personal data, customer data, private documents, or unrelated file contents. Use synthetic test values and redact screenshots and logs.

Please do not open a public issue for an undisclosed vulnerability. You can expect an initial acknowledgement within seven days. Timelines for validation, remediation, and coordinated disclosure depend on severity and reproducibility.

## Security Design Notes

- Supported credentials are stored through Windows Credential Manager.
- Persisted frontend state removes known API keys, connector secrets, webhook URLs, and MCP environment values.
- Audit and diagnostic paths redact common secret fields and token formats.
- High-impact local operations are expected to retain permission and approval checks.
- Release automation runs dependency and filesystem security scans and publishes checksums, an SBOM, and build attestations.

These controls reduce risk but do not make arbitrary tools, prompts, model providers, MCP servers, or plugins trustworthy. Only connect endpoints and install extensions you trust.

## Public Security Improvements

Hardening changes that do not disclose an exploitable vulnerability can use a normal pull request. If in doubt, report privately first.
