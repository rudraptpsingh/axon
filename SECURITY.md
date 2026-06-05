# Security Policy

## Supported Versions

Security fixes target the latest released version and the current `master`
branch.

## Reporting a Vulnerability

Please do not open a public issue for a vulnerability.

Use a private GitHub security advisory if available. If that is not possible,
contact the maintainer privately and include enough detail to reproduce and
assess the issue.

Include:

- affected version or commit
- operating system
- reproduction steps
- expected impact
- any known workaround or mitigation

## Privacy and Data Boundary

Axon must not add telemetry, analytics, or automatic outbound network calls.
The MCP server runs locally and exposes local runtime state only to the MCP
client that launched it, unless a user explicitly configures an outbound alert
webhook.

Security fixes should preserve the stdio contract: stdout is reserved for MCP
JSON-RPC, and logs belong on stderr.
