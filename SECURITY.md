# Security Policy

## Supported Versions

Security fixes are provided for the latest version of `axum-observability`
published on [crates.io](https://crates.io/crates/axum-observability). Older
releases, pre-1.0 releases, and unreleased commits are not supported. Upgrade
to the latest release before reporting a vulnerability when possible.

The supported Rust and Axum versions are documented in the
[README](README.md#requirements-and-installation). A problem caused solely by
an unsupported Rust or Axum version is outside this policy.

## Reporting a Vulnerability

Do not open a public issue for a suspected vulnerability. Use
[GitHub private vulnerability reporting](https://github.com/janisto/axum-observability/security/advisories/new)
instead.

Include enough information to reproduce and assess the report:

- the affected `axum-observability`, Rust, and Axum versions;
- enabled crate features and relevant configuration;
- a minimal reproduction or clear reproduction steps;
- the security impact, attack conditions, and affected data; and
- any known mitigation or proposed fix.

Use synthetic data. Do not include credentials, cookies, request or response
bodies, private logs, or other secrets.

Please allow up to seven days for an initial response. Accepted reports will be
handled privately while a fix and coordinated disclosure are prepared. If a
report is declined, the response will explain why. Do not disclose the issue
publicly before coordinated disclosure.

Report vulnerabilities that exist solely in Rust, Axum, Tower, `tracing`, or
another dependency to the affected upstream project. General bugs and
hardening suggestions without a security impact belong in the
[public issue tracker](https://github.com/janisto/axum-observability/issues).

This project does not currently offer a bug bounty.
