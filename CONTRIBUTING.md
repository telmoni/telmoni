# Contributing to Telmoni

Thanks for your interest in contributing to Telmoni! Telmoni is an open-source foundation for organizations and their projects: members and roles, API tokens, notifications, an audit log and a console agent.

All contributors are expected to adhere to our [Code of Conduct](CODE_OF_CONDUCT.md). For security vulnerabilities, please refer to our [Security Policy](SECURITY.md). For how Telmoni is built, see [architecture/](architecture/README.md); for setting up and running the stack, see [DEVELOPMENT.md](DEVELOPMENT.md).

---

## Developer Certificate of Origin (DCO)

We do not require a Contributor License Agreement (CLA). Instead, we use the standard [Developer Certificate of Origin (DCO)](https://developercertificate.org/).

By adding a `Signed-off-by:` line to your commit message, you certify that you have the right to submit the work under the project's [Apache-2.0 License](LICENSE).

Sign your commits using `git commit -s`:

```console
git commit -s -m "feat(auth): add session revocation endpoint"
```

---

## Guidelines & Principles

- **Open an issue first for major changes.** Before starting work on new features, schema changes, or significant architectural modifications, open an issue so design and scope can be aligned.
- **Rust is the Source of Truth:** All business logic, tenant isolation, database queries, RBAC authorization, and audit logging live in Rust (`crates/`). The modular monolith runs under one binary (`crates/telmoni`) communicating across clean library seams (`crates/shared/src/seam.rs`).
- **Console as a Thin Proxy:** The Next.js console (`web/`) is a backend-for-frontend (BFF). It never accesses the database directly and contains no business logic.
- **Strict Secrets and PII Protection:** Never log secrets or PII at any log level (including `debug` and `trace`). Sensitive values in Rust must be wrapped in `Redacted` (`crates/shared/src/types/redacted.rs`). Web console logging must use `@/lib/logger` with redaction enabled.
- **Pre-Release Schema Discipline:** Before `v0.1.0`, Telmoni does not maintain backwards compatibility or migration shims. Edit existing migration files directly under `crates/<module>/migrations/` and reset your local database with `make db-reset`.
- **Wire Contract Integrity:** After any intentional API wire change, regenerate contract files with `make contract`.

---

## Running Tests and Validation

Every change must pass our workspace CI task runner:

```console
make ci
```

`make ci` enforces:
- `make check` (Rust clippy, fmt, rustdoc, typecheck)
- `make lint` (ESLint across the web console)
- `make test TEST_THREADS=2` (vitest + cargo integration tests against local Postgres)
- `make web-gate` (web console production build and verification)
- `cargo deny check` (dependency security and license audit)
- `typos` (source code spelling check)

For quick local iteration, you can run individual checks:

```console
make check
make lint
make test TEST_THREADS=2
make fmt
```

---

## Submitting Pull Requests

1. **Keep Pull Requests Focused:** Submit PRs that address a single issue or feature.
2. **Commit Style:** Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `refactor:`, `chore:`).
3. **Sign Your Commits:** Ensure every commit includes the DCO sign-off (`-s`).
4. **No AI Signatures:** Do not include automated AI co-author or attribution tags in commits or PR bodies.
5. **Ensure Clean CI:** Verify that `make ci` passes cleanly before requesting review.
