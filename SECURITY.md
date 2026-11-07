# Security Policy

NEXUS ZERO is pre-alpha and has no released versions yet.

## Reporting a vulnerability

Please do **not** open a public issue for security problems. Use GitHub's private vulnerability reporting for this repository (Security tab, "Report a vulnerability"). Include affected files or components, reproduction steps, and the impact you observed.

You can expect an acknowledgement; response timelines will be documented once the project has a release process.

## Ground rules for this project

- Credentials, tokens, and API keys must never be committed. `.gitignore` excludes common secret files, and secret scanning will be added to CI.
- Provider tokens are server-side only and never sent to the frontend.
- Integrations request the minimum scopes needed and are read-only by default.
- Simulations never execute arbitrary user-provided code against production.
