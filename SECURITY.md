# Security policy

## Supported versions

Only the latest published version is maintained. This is an early-stage Windows desktop application; there is no security support SLA.

## Reporting a vulnerability

Do not post exploit details, private file paths or sensitive data in a public issue. Use the repository's **Security → Advisories → Report a vulnerability** entry when available:

https://github.com/Mingcodeingmiao/rust-explorer/security/advisories/new

If private reporting is unavailable, open an issue asking for a private contact channel without including vulnerability details. Include the affected version, Windows build, a minimal reproduction using disposable files, and the expected impact once a private channel is established.

## Scope and limitations

File operations run with the current user's permissions. Shell extensions and preview providers execute in helper processes for resilience; a helper process is **not** a security sandbox. Do not treat previewing or opening untrusted files as safe merely because a helper is used. Avoid running the application as administrator unless an operation requires it. Back up important files before testing new builds.

The application uses Windows Shell, installed providers and optional Everything integration. Report upstream component issues to their maintainers as well. Do not submit passwords, access tokens, full memory dumps or personal documents with a report.
