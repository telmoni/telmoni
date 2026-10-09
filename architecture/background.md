# Background work

Everything that runs other than in answer to a request is listed here. Most of it runs inside `telmoni serve`, as tasks in **every replica**. The rest runs as one-shot commands of the same binary:
- the chart schedules `migrate` and `rotate`;
- the sweeps run on demand.

Every piece of this work is safe for several replicas to run at once, and safe to stop at any point. A loop gets that safety in one of three ways:

- a **leader lock**, so only one replica runs it;
- a **lease on each row**, so replicas split the work;
- **idempotent transactions**, chunked where the work can be large, so running one twice does no harm.

## Contents

- [The map](#the-map)
- [Leadership](#leadership)
- [Leases](#leases)
- [Chunks](#chunks)
- [Inline work after a request](#inline-work-after-a-request)
- [Jobs](#jobs)
- [Stopping](#stopping)
- [Where it lives](#where-it-lives)

## The map

| Work | Runs | How often | Shared by | Details |
|---|---|---|---|---|
| Deletion sweep | `serve` (auth) | `DELETION_EVERY`, first after `START_DELAY` | Leader lock + heartbeat + budget | [deletion](deletion.md) |
| Audit verification | `serve` (auth) | `AUDIT_VERIFY_EVERY` | Leader lock + heartbeat + budget | [data](data.md#the-audit-log) |
| Auth retention | `serve` (auth) | `RETENTION_EVERY` | Nothing: one idempotent transaction | [data](data.md#retention) |
| Audit exports sweep | `serve` (auth) | `AUDIT_EXPORTS_EVERY` | Row leases, each write fenced on its attempt; the deletes are idempotent | [data](data.md#the-audit-log) |
| Delivery loop | `serve` (notifications) | `NOTIFICATIONS_DELIVERY_POLL_SECS` | Row leases, `SKIP LOCKED` | [notifications](notifications.md#the-delivery-queue) |
| Notifications retention | `serve` (notifications) | Hourly | An advisory lock per chunk | [notifications](notifications.md#retention) |
| Agent indexer | `serve` (agent, when on) | `EVERY`, first after its own `START_DELAY` | Cursor leases | [agent](agent.md#the-index) |
| Agent retention | `serve` (agent, whenever linked) | Hourly | Advisory locks per pass | [agent](agent.md#retention) |
| First delivery attempts | Spawned after an emit commits | Per notice | The delivery row's lease | [notifications](notifications.md#raising-a-notice) |
| Audit export builds | Spawned after a request queues one | Per export, two at once a process | The export row's lease | [data](data.md#the-audit-log) |
| Migrations | `telmoni migrate`, a Helm hook Job | Each install or upgrade | One Job | [data](data.md#migrations) |
| Partition rotation | `telmoni rotate`, a CronJob | Daily | `concurrencyPolicy: Forbid` | [data](data.md#partitions) |
| On demand | `telmoni sweep deletion\|audit-verify\|retention\|agent-reindex\|audit-exports` | By hand or by a scheduler | `deletion` and `audit-verify` take the loops' leader locks. `retention`, `agent-reindex` and `audit-exports` take none. | [server](server.md#subcommands) |

Notes on the map:
- **Auth's four sweeps start unless `RUN_SWEEPS=false`.** A deployment that sets it schedules `telmoni sweep …` runs elsewhere instead (`crates/telmoni/src/sweeps.rs`); without `audit-exports` among them, a file outlives its week and a build a restart dropped is never finished.
- **Most timers delay rather than stack.** Auth's sweeps, the delivery loop and the indexer push the next tick back after a slow one. The two hourly retention loops use tokio's default, so ticks missed during a slow sweep run back to back. A failed tick is logged, and the next one retries.
- **Notifications' loops always start.** The agent's retention starts whenever the agent has a database (`AGENT_DATABASE_URL`). Its indexer starts only when the agent is on.
- **Agent retention runs even with the model off**, so what an earlier configuration indexed still ages out, and erasures still reach it.

## Leadership

Auth's deletion sweep and audit verification must not overlap between replicas. Each tick elects a leader for itself (`crates/auth/src/sweep.rs`):

1. **Open a transaction** of its own and try `pg_try_advisory_xact_lock(<sweep's lock id>)`. A replica that does not get the lock skips the tick.
2. **Run the tick's work inside `as_leader`.** While it runs, `as_leader` pings the lock's transaction (`SELECT 1`) every `LEADER_HEARTBEAT`.
3. **Roll the lock's transaction back** when the work ends, which releases the lock.

Why it is built this way:

- ⚠ **The heartbeat.** The lock lives on a transaction that sits idle while the work runs on other connections. The module roles' `idle_in_transaction_session_timeout` kills a transaction idle for too long. Before the heartbeat, a tick that ran past that cut-off lost its lock halfway, and another replica's tick ran beside the rest of it.
- **A leader that dies stops pinging.** If its connection closes, the lock is freed at once. If the connection is cut off without closing, the idle cut-off frees it.
- **A ping that fails ends the tick.** The lock may already be gone.
- ⚠ **The budget** (`DELETION_TICK_BUDGET`, `AUDIT_VERIFY_BUDGET`) ends a tick that runs too long. A step hung on an outside call, such as an identity provider or a purge hook, would otherwise keep the lock pinged forever, and stop every replica's sweep. The next tick carries on, since every step is safe to repeat.
- **Within a tick**, a failed item (one organization, one person) is logged and does not stop the others. The run still reports failure.

## Leases

Work too long for one transaction is split by leasing rows rather than holding locks:

- **Deliveries.** One statement claims due rows with `FOR UPDATE SKIP LOCKED`, counts the attempt, and sets `lease_until`. It commits before anything is sent. A replica that dies mid-send leaves the row leased, and once the lease lapses another replica sends it again. So delivery is at least once, and receivers dedupe on the delivery id.
- **The agent's cursors.** Each source's cursor row is leased to one replica while it embeds a page. Embedding can take minutes, and no transaction stays open across that.
  - The lease is renewed alongside the work.
  - A page whose lease was lost is dropped, and read again by whoever holds the cursor.
  - An erasure or a purge revokes the leases, so a page read before the scrub cannot land after it.

## Chunks

Notifications' retention, the agent's retention and the agent's organization purge delete in bounded chunks, one transaction each:

- ⚠ **Every module role has a short `statement_timeout`.** An unbounded delete on a large tenant outlasted it on every attempt and never finished.
- **Each chunk takes a transaction-scoped advisory lock**, such as `notifications:retention`, `agent:retention` or `agent:projects`. A replica that finds the lock held leaves that pass to whoever holds it.
- **What was committed stays committed.** A sweep that stops halfway has still made progress, and the next run continues.

Some deletes are not chunked, and each is one transaction:
- **auth's retention**, whose deletes have no limit;
- **notifications' organization and project purges.**

A backlog large enough to outlast the role's statement timeout rolls such a run back whole, and the next run starts again.

## Inline work after a request

Some work starts with a request but outlives the reply:

- **First delivery attempts** are spawned after the emit commits, so the caller never waits on a vendor. A per-process limit applies, and anything over it waits for the loop. ⚠ They are never cancelled mid-send: a timeout once dropped a send after the far end had the message, and it was sent twice.
- **The deletion tail** after an organization or account deletion request is **awaited, never spawned**, because a spawned tail would die with the pod. It is bounded by `DELETION_TAIL_BUDGET_MS`, under the console's own timeout. Whatever it does not finish, the deletion sweep finishes.
- **The agent's erase** runs in a task the request awaits. If the caller gives up, it still finishes.
- **An agent turn** runs in a task spawned behind its event stream. It stops at its next check once the stream is closed.
- **The password-reset mail** is a bare spawned task, on purpose: the response must not wait on it, or its timing would reveal which addresses have accounts. A rollout can lose it, and nothing retries it; the person asks again.
- **A raised log level's revert** is a timer task, per pod (see [server](server.md#logging)).

## Jobs

The same image runs these commands:

| Command | Scheduled by | Notes |
|---|---|---|
| `migrate` | A Helm `pre-install,pre-upgrade` hook Job. Compose runs it as a one-shot service. | Runs before the new server starts. It needs only the migrator's credentials and the CA certificate. |
| `rotate` | A CronJob, daily | DDL as the migrator. It is a job, never part of `serve`, because the serving roles must not own tables. |
| `sweep …` | Nothing by default. Run by hand, or scheduled when `RUN_SWEEPS=false`. | Builds the whole process and runs one tick |

## Stopping

`serve` drains HTTP on `SIGTERM` or `SIGINT`. The loops are not told to stop; they end with the process. None of them loses work, because none of them depends on finishing:

| Loop | What happens to work in flight |
|---|---|
| Delivery | A leased row is sent again after its lease. |
| Auth's sweeps | The leader lock dies with its connection, and every step is idempotent. |
| Retention sweeps | Notifications' and the agent's committed chunks stay committed. Auth's single transaction rolls back and runs again next time. |
| Agent indexer | The cursor moves only when a page is written, and the lease lapses. |
| Audit export builds | A build cut short leaves its export `running` until its 10-minute lease lapses; the exports sweep after it builds it again, on one of its three attempts. The file and its `exported` record commit together, so nothing is half written. |

⚠ **The delivery loop is raced against the HTTP server.** If the loop ever ends, the process exits, and the restart is the recovery. A Ready pod whose delivery loop had died would leave every delivery pending. The retention sweeps are left detached on purpose: a stalled sweep only keeps rows longer than it should.

## Where it lives

| Concern | File |
|---|---|
| Starting the loops | `crates/telmoni/src/lib.rs` (`serve`), `crates/telmoni/src/sweeps.rs` |
| Leader election, budgets, the deletion and audit ticks | `crates/auth/src/sweep.rs` |
| Delivery loop | `crates/notifications/src/delivery.rs` |
| Notifications retention | `crates/notifications/src/retention.rs` |
| Agent indexer and leases | `crates/agent/src/index/mod.rs` |
| Agent retention | `crates/agent/src/retention.rs` |
| Jobs in the chart | `deploy/charts/telmoni/templates/migrator.yaml` |
