# Support

LocalAI Cowork is early-stage open-source software. Community support happens through GitHub.

## Before Opening An Issue

1. Check the [latest release notes](https://github.com/noshitcoding/LocalAI-Cowork/releases/latest).
2. Search [existing issues](https://github.com/noshitcoding/LocalAI-Cowork/issues).
3. Run the project doctor when using a source build:

   ```powershell
   cd app
   npm run doctor
   ```

4. Reduce the problem to the smallest reproducible workflow.

## Where To Ask

- [Bug report](https://github.com/noshitcoding/LocalAI-Cowork/issues/new?template=bug_report.yml) for reproducible incorrect behavior
- [Feature request](https://github.com/noshitcoding/LocalAI-Cowork/issues/new?template=feature_request.yml) for a concrete user problem or improvement
- [Privacy question](https://github.com/noshitcoding/LocalAI-Cowork/issues/new?template=privacy_question.yml) for non-sensitive data-handling questions
- [Private security report](https://github.com/noshitcoding/LocalAI-Cowork/security/advisories/new) for vulnerabilities or suspected credential exposure

## Safe Diagnostic Sharing

Before posting logs, screenshots, configuration, or support bundles, remove:

- API keys, bearer tokens, passwords, cookies, webhook URLs, and environment secrets
- Prompts, responses, and file contents that are not essential to reproduction
- Usernames, email addresses, personal folders, private repository names, and network addresses
- Provider account IDs, organization IDs, and billing details

Use synthetic values such as `sk-test`, `https://example.test`, and `C:\Users\example\workspace`. Never post a real credential even if you plan to revoke it later.

## Scope

The maintained release target is currently Windows 10 and Windows 11. Source builds on other platforms may work but are not yet part of the supported release and smoke-test matrix.
