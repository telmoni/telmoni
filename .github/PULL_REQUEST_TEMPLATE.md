## Description

Please include a summary of the change, related issues, and context.

Fixes #(issue)

## Type of change

- [ ] Bug fix (non-breaking change fixing an issue)
- [ ] New feature (non-breaking change adding functionality)
- [ ] Breaking change (fix or feature causing existing functionality to not work as expected)
- [ ] Database migration / schema change
- [ ] Helm chart / deployment configuration update
- [ ] Documentation update
- [ ] Maintenance / CI / Tooling

## Component

- [ ] Web Console (`web/`)
- [ ] Server / API (`crates/telmoni`)
- [ ] Auth & RBAC (`crates/auth`)
- [ ] AI Agent (`crates/agent`)
- [ ] Notifications & Webhooks (`crates/notifications`)
- [ ] Shared Library & Database primitives (`crates/shared`)
- [ ] Database Migrator (`crates/migrator`)
- [ ] Helm Chart & Kubernetes (`deploy/charts/telmoni`)
- [ ] Docker Compose & Local Environment (`deploy/compose`)

## Checklist

- [ ] `make ci` passes locally (`cargo test` and `npm --prefix web test`)
- [ ] Tests covering new logic or bug fixes are included
- [ ] Database migrations are backwards-compatible and follow zero-downtime rules
- [ ] Environment variables or configuration changes are updated in `.env.example` and Helm `values.yaml` where appropriate
- [ ] Code adheres to `#![forbid(unsafe_code)]` and workspace clippy deny rules
