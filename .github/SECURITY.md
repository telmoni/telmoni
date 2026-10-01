# Security Policy

## Supported Versions

Telmoni is currently pre-release. Security patches are applied to the active development branch (`main`).

| Version | Supported          |
| ------- | ------------------ |
| `main`  | :white_check_mark: |
| < 0.1.0 | :white_check_mark: |

Once versioned releases are tagged (starting with `v0.1.0`), this table will reflect supported release lines and backport windows.

## Scope

Security vulnerability reports are welcome for:
- The Rust server and its crates (`crates/telmoni`, `auth`, `notifications`, `shared`, `migrator`).
- The Next.js web console (`web/`).
- Helm charts and reference Docker deployment configurations (`deploy/`).
- Self-hosted installations as well as the reference hosted deployment.

Issues in third-party dependencies are evaluated based on exploitability within Telmoni's architecture.

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues, pull requests, or discussions.**

We offer two private channels for reporting:

1. **GitHub Private Vulnerability Reporting (Preferred):**  
   Use GitHub's [Private Vulnerability Reporting](https://github.com/telmoni/telmoni/security/advisories/new) under the repository's **Security > Advisories** tab.

2. **Email:**  
   Send details to **hello@telmoni.com**.

When reporting, please provide:
- A clear description of the vulnerability and its potential impact.
- Step-by-step reproduction instructions or a minimal proof of concept (PoC).
- The commit SHA, version, or deployment topology tested against.
- Any proposed remediation, if known.

## Response & Disclosure Timeline

- **Initial Response:** We will acknowledge receipt of your report within three working days.
- **Triage & Status:** We will keep you informed of our investigation, confirmation status, and planned timeline for shipping a fix.
- **Coordinated Disclosure:** We request that you give us a reasonable window to remediate the vulnerability before public disclosure.
- **Bug Bounty:** There is currently no paid bug bounty programme.

## Testing Guidelines & Safe Harbor

When testing for vulnerabilities:
- Test only against instances or accounts you own and control.
- Do not attempt to access, view, or modify data belonging to other accounts.
- Do not degrade the performance of any shared or hosted services (e.g. avoid denial-of-service tests or brute-force attacks).

We consider vulnerability research conducted in compliance with these guidelines to be authorized and will not initiate legal action against researchers acting in good faith.
