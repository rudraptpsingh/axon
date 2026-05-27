# Security Policy

## Supported Versions

Security fixes target the latest released version and the current `master`
branch.

## Reporting a Vulnerability

Please do not open a public issue for a vulnerability. Email the maintainer or
use a private GitHub security advisory if available.

Include:

- affected version or commit
- operating system
- reproduction steps
- impact
- any suggested mitigation

## Privacy and Data Boundary

Axon must not add telemetry, analytics, or automatic outbound network calls.
The MCP server runs locally and exposes local runtime state only to the MCP
client that launched it.
