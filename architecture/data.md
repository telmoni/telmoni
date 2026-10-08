# Data

Telmoni keeps all of its state in **one PostgreSQL 17 database**, with one schema per module:

| Schema | Owned by | Holds |
|---|---|---|
| `auth` | auth | People, identities, sessions and tokens; organizations, projects, rosters and invitations; API tokens; flags |
| `notifications` | notifications | The feeds, connections, the delivery queue and its attempts |
| `agent` | agent | Passages and their vectors, conversations, the indexer's cursors, erasure fences |
| `audit` | the migrator, written through `emit_audit` | The hash-chained audit log, partitioned by month |

The `vector` extension (pgvector 0.8 or later) lives in `public`, because the agent's column type needs it.

**The console holds no data of its own.** Its Redis keeps only rate-limit windows, a session blacklist, an announcement and a live-event bus, all of which can be lost (see [the console](console.md#redis)).

This page covers migrations, partitions, the audit log and retention. Who may read which rows is in [tenancy](tenancy.md). How data leaves when an organization or a person is deleted is in [deletion](deletion.md).

## Contents

- [Migrations](#migrations)
- [Partitions](#partitions)
- [The audit log](#the-audit-log)
- [Retention](#retention)
- [Where it lives](#where-it-lives)

## Migrations

**One migration file per module, edited in place.** Nothing has shipped, so a schema change edits the module's single `*_initial.sql` and never adds a file; a local database is rebuilt with `make db-reset`.

| Set | File |
|---|---|
| `audit` | `crates/migrator/migrations/20261001100000_audit_initial.sql` |
| `auth` | `crates/auth/migrations/20261001100000_auth_initial.sql` |
| `notifications` | `crates/notifications/migrations/20261001100000_notifications_initial.sql` |
| `agent` | `crates/agent/migrations/20261001100000_agent_initial.sql` |

**`telmoni migrate`** (`crates/migrator/src/lib.rs`) connects only as `MIGRATOR_DATABASE_URL`. It never builds the server.

1. **Find the sets.** It reads `MIGRATION_SETS` (`schema=dir,…`, owned by the Makefile), or else every directory under `MIGRATIONS_ROOT`. The image copies each module's directory there. The whole list is validated before anything runs.
   - ⚠ Schema names must be bare lower-case identifiers. They are pasted into `CREATE SCHEMA` and `SET search_path`, which take no bind parameters.
2. **Require the core.** The `audit` and `auth` sets must be present. A partial run would leave a server booting against a database it cannot write.
3. **Run in a fixed order:** `audit`, then `auth`, then the rest by name.
4. **Give each set its own schema and ledger.** Each set runs with `search_path` set to its own schema, so each schema keeps its own `_sqlx_migrations` ledger.
5. **Apply the object grants** (`crates/migrator/sql/object_grants.sql`). The file is compiled into the binary, so the grants cannot drift from the binary that applies them.
   - If any module login role is missing (a single-role deployment), the grants are skipped with a warning.
   - Extra grant files can follow, from `MIGRATION_GRANTS` or `/app/grants/`. A deployment uses these for its own modules. Each names its grantees on its first line, and is skipped if one of them is missing.

**What the migrations themselves do** beyond tables:
- They create their module's maintenance lane role, and grant the lane what it needs.
- They enable and force row-level security, and write the policies.
- The audit migration creates the first months of partitions from the clock, because a listed run of months eventually runs out.
- ⚠ The agent's migration refuses to run without `vector` 0.8 or later. The `migrator` role cannot create an extension, so a superuser installs it once per database. Locally, `crates/migrator/sql/dev_extensions.sql` installs it in `telmoni` and `template1`, for test databases.

**Superuser setup**, outside `migrate`:
- `crates/migrator/sql/role_hardening.sql` hardens the four login roles, sets their session defaults and creates the lane roles. It is idempotent. See [tenancy](tenancy.md#database-roles).
- **Locally**, the dev compose runs the extensions, `dev_roles.sql` and the hardening when the volume is first created.
- **`make db-reset`** drops the volume, starts Postgres again, waits, migrates, and pins the local superuser's `search_path`. It does not run `rotate`; the migration's own partitions cover a fresh database.

**In a deployment**, the chart runs `migrate` as a Helm pre-install and pre-upgrade hook, before the new server starts. Compose runs it as a one-shot service that the server waits on. See [deploy](deploy.md#ordering).

## Partitions

**Only `audit.events` is partitioned**: by range on `created_at`, one child per month (`events_YYYY_MM`), with no default partition.

⚠ **When the partitions run out, every write fails.** An insert past the last partition errors. `emit_audit` runs inside the business transaction, so that takes every mutating operation down with it. Partitions are therefore created months ahead.

**`telmoni rotate`** (`crates/migrator/src/rotate.rs`) creates the current month through `BUFFER_MONTHS` ahead (`CREATE TABLE IF NOT EXISTS … PARTITION OF`), then hardens each new child.
- ⚠ **Each child gets its own policies.** A query that names a child directly does not go through the parent's policies.
  - Children that are already hardened are skipped, because the `ALTER` takes an exclusive lock on a live partition.
  - Grants stay on the parent alone.
- ⚠ **It runs with a short `lock_timeout`.** While its DDL waits for a lock, every audit insert queues behind it. A run that gives up costs nothing, because months are already created ahead.
- It runs as the migrator, because creating a partition needs ownership of the parent, which no request-facing role may hold.
- It stops at the first error, and is safe to run again.

**Nothing is ever dropped today.** The partition registry (`RETENTION`, `crates/shared/src/db/retention.rs`) gives `audit.events` a retention window, but sets `drop_enabled: false` until a cold archive exists, so history is never destroyed. `rotate` does have the drop path wired in, and runs it for any table whose registry entry turns `drop_enabled` on.

**Schedule.**
- The chart runs `rotate` as a CronJob, with concurrency forbidden.
- Compose and local setups run it by hand.
- `serve` never rotates.

## The audit log

**Who writes it.** `emit_audit` (`crates/shared/src/audit.rs`) is the only writer.
- ⚠ **It runs inside the handler's own transaction.** The audit row commits with the change it records. A failed audit write fails the operation, and a failed operation leaves no orphan row. That is why it is a helper called in the transaction, not middleware: by the time middleware sees a response, the transaction has committed.
- **Nothing else can write.** `crates/shared/tests/append_only_audit.rs` fails CI on any other insert, update or delete. The database grants allow only `SELECT` and `INSERT`. The RBAC matrix refuses every write verb on Audit.
- **The callers** are auth's handlers and sweeps, and notifications' connector lanes. The agent writes nothing to the chain.

**What a row holds:**
- the organization, and `in_project` when the act happened inside a project;
- the actor:
  - a user id;
  - `service:<name>` (the operator, auth, the deletion saga);
  - `external:slack`;
- the action: Created, Updated, Deleted or Exported;
- the resource kind and id;
- JSON metadata;
- the time;
- `seq` and the chain hashes.

The `ip_address` and `user_agent` columns exist, but nothing fills them.

**The chain.** There is **one chain per organization**. A project has no chain of its own; its events are rows of its organization's chain, marked `in_project`.

To write a row, `emit_audit`:
1. **Binds the organization** for the insert, whenever `app.organization_id` is empty, as it is in a project-scoped transaction, and clears it after.
2. **Takes an advisory lock on the organization.**
   - ⚠ The lock is per organization, not per project. A per-project lock would let two projects' writes fork the chain.
   - The `(organization_id, seq)` index is not unique, so this lock is what keeps the chain linear.
3. **Reads the newest row by `seq`**, never within a time window. ⚠ A window on `created_at` once minted a duplicate `seq` at a month boundary.
4. **Writes the next row.** `seq` is one more than the newest, and `prev_hash` is the newest row's hash. The id is a UUIDv7. `created_at` comes from the application's clock, because it is an input to the hash.

**`row_hash`** (`crates/shared/src/db/audit_hash.rs`) is a SHA-256 over length-prefixed segments, in this order:
1. id, organization, actor, action, resource kind, resource id;
2. the metadata as canonical JSON (keys sorted);
3. the time;
4. `prev_hash`;
5. `in_project`, appended only when present.

The rules of the hash:
- An absent field is tagged so it differs from an empty one.
- `in_project` is the one exception to fixed segments. Appending it only when present keeps rows outside a project hashing the same as they always did.
- ⚠ **The format is a frozen contract, pinned by golden vectors.** Changing the inputs or their order invalidates every historical hash.
- `seq`, `request_id`, `ip_address`, `user_agent` and `shard_key` are not hashed.

**Verification** (`crates/shared/src/db/audit_verify.rs`) walks a chain by `seq`, so it never depends on clocks. It reads a page at a time.
- ⚠ It pages because reading a whole history into memory risked running out of memory.
- Each row's hash is recomputed. A mismatch means a field was edited in place.
- Each link is checked. A mismatch means a row was deleted, reordered or inserted.
- A chain missing its first row fails.

The daily `audit-verify` sweep walks every organization's chain under a statement timeout of its own:
- **A break** is logged as `AUDIT CHAIN BREAK` and never repaired: a forked chain is a finding.
- **A chain that could not be read** is reported as unchecked, never as intact.
- Either one fails the run, but only after every chain has been walked.

**Reading.**
- Owners and admins read a project's events, and the organization's whole log, in the console. The filters are typed, so a misspelled filter is a 400 rather than an empty log.
- The agent indexes events only once they are older than a settling delay (`INDEX_SETTLE_SECONDS`). A row is stamped by `emit_audit` before its transaction commits. A commit that lands late would otherwise put the row behind the indexer's cursor. The agent's copy leaves out the metadata's top-level `email`.
- The export carries the newest events, without hashes (see [deletion](deletion.md#export)).

**The audit log outlives what it describes.**
- Rows stay after their project or organization is deleted, because `in_project` has no foreign key.
- Rows naming an erased person stay, by user id.

## Retention

Each kind of data has a window. The windows that are published (in the privacy policy, for example) are marked ⚠ in the code beside them.

| Data | Window | Where |
|---|---|---|
| `audit.events` | A window is declared, but partitions are never dropped today | `crates/shared/src/db/retention.rs` |
| Feed rows | `FEED_RETENTION_DAYS`, by `created_at` | `crates/shared/src/db/retention.rs` |
| Finished deliveries, and their attempts | `DELIVERY_RETENTION_DAYS`, by `updated_at`. Rows a resend holds are skipped. | `crates/shared/src/db/retention.rs` |
| Connector handshake states | At expiry | `crates/notifications/src/db.rs` |
| A rotated webhook secret | When its overlap ends | `crates/notifications/src/db.rs` |
| The agent's copies of audit events, feed rows and deliveries | Their sources' windows, imported, so a copy never outlives its source | `crates/agent/src/retention.rs` |
| The agent's docs passages | While the docs corpus has the page | `crates/agent/src/index/docs.rs` |
| Agent conversations | ⚠ `CONVERSATION_RETENTION_DAYS` after last use (published) | `crates/agent/src/retention.rs` |
| Agent erasure fences | `ERASURE_FENCE_DAYS` after last use | `crates/agent/src/retention.rs` |
| API tokens | Some days after revocation or expiry | `crates/auth/src/db/tokens.rs` |
| Invitations | Unaccepted: some days after expiry or revocation. Accepted: until the person who accepted is erased. | `crates/auth/src/db/invites.rs` |
| Sessions | Revoked sessions, after some days. Live sessions stay. | `crates/auth/src/db/sessions.rs` |
| Bearers, sign-in codes, device codes | At expiry | `crates/auth/src/db/` |
| Refresh tokens | At expiry, or `SPENT_RETENTION` after use | `crates/auth/src/db/refresh_tokens.rs` |
| Confirmation codes | Valid for `CODE_TTL_MINUTES`. Old rows are pruned when the person is issued a new code. | `crates/auth/src/db/confirmation_codes.rs` |
| An organization pending deletion | `FINALIZE_GRACE_SECONDS`, whoever asked: fifteen minutes, then the sweep erases it. | `crates/auth/src/handler/deletion.rs` |

Each module's own sweep applies its windows (see [background work](background.md)):
- **Notifications' and the agent's sweeps** delete in bounded chunks, because the module role's statement timeout would end an unbounded delete.
- **Auth's retention** runs its deletes in one transaction, without limits. A run that hits the statement timeout rolls back whole and is tried again on the next run.

## Where it lives

| Concern | File |
|---|---|
| Migration runner, sets, grants | `crates/migrator/src/lib.rs` |
| Rotation | `crates/migrator/src/rotate.rs`, `crates/shared/src/db/retention.rs` |
| Roles, grants, local setup | `crates/migrator/sql/` |
| Audit schema | `crates/migrator/migrations/20261001100000_audit_initial.sql` |
| Writing the audit log | `crates/shared/src/audit.rs` |
| The chain's hash, verifying it | `crates/shared/src/db/audit_hash.rs`, `crates/shared/src/db/audit_verify.rs`, `crates/shared/src/digest.rs` |
| Reading the audit log | `crates/auth/src/handler/audit.rs`, `crates/auth/src/db/audit.rs` |
| Append-only guard | `crates/shared/tests/append_only_audit.rs` |
| Retention windows | `crates/shared/src/db/retention.rs`, and each module's `retention.rs` or `db/` |
| Make targets (`db-reset`, `db-migrate`, `MIGRATION_SETS`) | `Makefile` |
