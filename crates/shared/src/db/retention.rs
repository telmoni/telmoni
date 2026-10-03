//! Partition-rotation planning — pure, no DB, no privilege.
//!
//! `audit.events` is RANGE-partitioned by month. Once the runway of created
//! partitions ends, an INSERT errors — and because `emit_audit` runs inside the
//! business transaction, that takes every mutating operation down with it. This
//! plans which months to **create ahead** and which to **drop**, and renders
//! the DDL.
//!
//! Pure because executing it needs to OWN the parent, which cannot be granted
//! to a request-facing role; it runs as `migrator` in rotate mode. Identifiers
//! come from a fixed-width year and month, so the DDL has no injection surface.

use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};

use super::tenant_session::MaintenanceLane;

/// A month-RANGE-partitioned table and its retention policy.
#[derive(Debug, Clone, Copy)]
pub struct PartitionedTable {
    /// Owning schema (`audit`).
    pub schema: &'static str,
    /// Parent table name (`events`).
    pub table: &'static str,
    /// Retention window in days. A partition is droppable once its *entire*
    /// month range is older than `now - retention_days`.
    pub retention_days: u32,
    /// Whether the drop side is active. Off for `audit` until a cold archive
    /// exists, so history is never destroyed.
    pub drop_enabled: bool,
    /// The RLS key column, and via `app.<column>` the GUC each child's policy
    /// compares. Hardcoding one key would harden a table keyed by the other
    /// with a predicate on a column it does not have — every direct read
    /// refused, maintenance included.
    pub tenant_key: &'static str,
    /// The maintenance lanes the parent's `maintenance_access` policy admits.
    /// Each child carries the same list, so it must match the parent's
    /// migration exactly — a lane missing here loses the child's rows when a
    /// query names it directly.
    pub lanes: &'static [MaintenanceLane],
}

/// How long the feed keeps a notice: a recent-activity surface, not an
/// archive. A wrapped negative day count would make a future cut-off, so i32
/// is preserved. The agent index keeps its copies of the feed to this window too.
pub const FEED_RETENTION_DAYS: i32 = 90;

/// How long a finished delivery row stays, with its body and its sends: the
/// delivery log window, and how far back a resend can reach. The agent index
/// keeps its copies of deliveries to this window too.
pub const DELIVERY_RETENTION_DAYS: i32 = 30;

/// The rotation registry. Every parent must exist in the migrated schema:
/// `rotate()` aborts at the first failing DDL, so a phantom entry starves
/// create-ahead for everything after it.
pub const RETENTION: &[PartitionedTable] = &[PartitionedTable {
    schema: "audit",
    table: "events",
    retention_days: 730,
    drop_enabled: false,
    tenant_key: "organization_id",
    lanes: &[MaintenanceLane::Auth, MaintenanceLane::Notifications],
}];

/// Keep this many months of partitions ahead of the current month, so a clock
/// skew or a missed nightly run still has runway before INSERTs fall off.
pub const BUFFER_MONTHS: u32 = 3;

/// One monthly partition, identified by calendar year + month.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionMonth {
    /// Calendar year.
    pub year: i32,
    /// Month, `1..=12`.
    pub month: u32,
}

impl PartitionMonth {
    /// The month containing `dt`.
    #[must_use]
    pub fn containing(dt: DateTime<Utc>) -> Self {
        Self {
            year: dt.year(),
            month: dt.month(),
        }
    }

    /// The month `n` months after this one (wraps years correctly).
    #[must_use]
    pub fn plus_months(self, n: u32) -> Self {
        debug_assert!(
            (1..=12).contains(&self.month),
            "PartitionMonth.month out of range: {}",
            self.month,
        );
        let total = self.month.saturating_sub(1) + n;
        let year = self.year + i32::try_from(total / 12).unwrap_or(0);
        let month = total % 12 + 1;
        Self { year, month }
    }

    /// Child-partition suffix, `YYYY_MM` (fixed width — no injection surface).
    #[must_use]
    pub fn suffix(self) -> String {
        format!("{:04}_{:02}", self.year, self.month)
    }

    /// Inclusive lower bound, `YYYY-MM-01`.
    #[must_use]
    pub fn lower(self) -> String {
        format!("{:04}-{:02}-01", self.year, self.month)
    }

    /// Exclusive upper bound — the first day of the following month.
    #[must_use]
    pub fn upper(self) -> String {
        let n = self.plus_months(1);
        format!("{:04}-{:02}-01", n.year, n.month)
    }

    /// First instant of this month (its inclusive lower bound as a timestamp).
    fn start_instant(self) -> Option<DateTime<Utc>> {
        Utc.with_ymd_and_hms(self.year, self.month, 1, 0, 0, 0)
            .single()
    }
}

/// Months that must have a partition: the current month through `buffer_months`
/// ahead. Re-running is idempotent; the DDL is `IF NOT EXISTS`.
#[must_use]
pub fn months_to_create(now: DateTime<Utc>, buffer_months: u32) -> Vec<PartitionMonth> {
    let base = PartitionMonth::containing(now);
    (0..=buffer_months).map(|n| base.plus_months(n)).collect()
}

/// `CREATE TABLE IF NOT EXISTS <schema>.<table>_YYYY_MM PARTITION OF
/// <schema>.<table> FOR VALUES FROM ('lower') TO ('upper')`.
#[must_use]
pub fn create_ddl(t: &PartitionedTable, m: PartitionMonth) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {schema}.{table}_{suffix} \
         PARTITION OF {schema}.{table} FOR VALUES FROM ('{lo}') TO ('{hi}')",
        schema = t.schema,
        table = t.table,
        suffix = m.suffix(),
        lo = m.lower(),
        hi = m.upper(),
    )
}

/// The RLS hardening a freshly created child partition needs, as one idempotent
/// `DO` block.
///
/// ⚠ **A child needs its own policies.** A parent's policies apply only to
/// queries routed through the parent; a query naming the child directly is
/// governed by the child's, and a plain `PARTITION OF` child has none — so it
/// reads every tenant's rows. Verified empirically, not assumed. Grants on the
/// parent only are the other half of the defence, and both are kept: a future
/// `GRANT … ON ALL TABLES` would reopen the hole without these.
///
/// Policies are guarded by a `pg_policy` lookup because `CREATE POLICY` has no
/// `IF NOT EXISTS`.
#[must_use]
pub fn harden_child_ddl(t: &PartitionedTable, m: PartitionMonth) -> String {
    let child = format!("{}_{}", t.table, m.suffix());
    format!(
        "DO $$
BEGIN
    -- Only when not already so: the ALTER takes an ACCESS EXCLUSIVE lock on
    -- the partition taking this month's writes, and every audited mutation
    -- queued behind it while rotate waited on a reader.
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c
          JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = '{schema}' AND c.relname = '{child}'
           AND c.relrowsecurity AND c.relforcerowsecurity
    ) THEN
        EXECUTE 'ALTER TABLE {schema}.{child} ENABLE ROW LEVEL SECURITY';
        EXECUTE 'ALTER TABLE {schema}.{child} FORCE ROW LEVEL SECURITY';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_policy p
          JOIN pg_class c     ON c.oid = p.polrelid
          JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = '{schema}' AND c.relname = '{child}'
           AND p.polname = 'tenant_isolation'
    ) THEN
        EXECUTE 'CREATE POLICY tenant_isolation ON {schema}.{child} '
             || 'USING ({key} = current_setting(''app.{key}'', true)) '
             || 'WITH CHECK ({key} = current_setting(''app.{key}'', true))';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_policy p
          JOIN pg_class c     ON c.oid = p.polrelid
          JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = '{schema}' AND c.relname = '{child}'
           AND p.polname = 'maintenance_access'
    ) THEN
        EXECUTE 'CREATE POLICY maintenance_access ON {schema}.{child} '
             || 'TO {lane_roles} USING (current_user IN ({lane_names})) '
             || 'WITH CHECK (current_user IN ({lane_names}))';
    END IF;
END $$;",
        key = t.tenant_key,
        schema = t.schema,
        child = child,
        lane_roles = t
            .lanes
            .iter()
            .map(|l| l.role())
            .collect::<Vec<_>>()
            .join(", "),
        lane_names = t
            .lanes
            .iter()
            .map(|l| format!("''{}''", l.role()))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// Whether the partition for `m` is fully past `t`'s retention window at `now`:
/// its exclusive upper bound is at or before `now - retention_days`, so every
/// row in it is older than the window.
#[must_use]
pub fn is_expired(t: &PartitionedTable, m: PartitionMonth, now: DateTime<Utc>) -> bool {
    match m.plus_months(1).start_instant() {
        Some(upper) => upper <= now - Duration::days(i64::from(t.retention_days)),
        None => false,
    }
}

/// `DROP TABLE IF EXISTS <schema>.<table>_YYYY_MM`.
#[must_use]
pub fn drop_ddl(t: &PartitionedTable, m: PartitionMonth) -> String {
    format!(
        "DROP TABLE IF EXISTS {schema}.{table}_{suffix}",
        schema = t.schema,
        table = t.table,
        suffix = m.suffix()
    )
}

/// Parse a child-partition relname (`events_2026_05`) back to its month.
/// `None` for anything not shaped exactly `<table>_YYYY_MM`, so a stray
/// same-prefixed table is never mistaken for a partition and dropped.
#[must_use]
pub fn parse_child(table: &str, relname: &str) -> Option<PartitionMonth> {
    let suffix = relname.strip_prefix(table)?.strip_prefix('_')?;
    let (y, mo) = suffix.split_once('_')?;
    if y.len() != 4 || mo.len() != 2 {
        return None;
    }
    let year = y.parse::<i32>().ok()?;
    let month = mo.parse::<u32>().ok()?;
    (1..=12)
        .contains(&month)
        .then_some(PartitionMonth { year, month })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, mo: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, 12, 0, 0).single().unwrap()
    }

    #[test]
    fn plus_months_wraps_year_boundary() {
        let nov = PartitionMonth {
            year: 2026,
            month: 11,
        };
        assert_eq!(
            nov.plus_months(1),
            PartitionMonth {
                year: 2026,
                month: 12
            }
        );
        assert_eq!(
            nov.plus_months(2),
            PartitionMonth {
                year: 2027,
                month: 1
            }
        );
        assert_eq!(
            nov.plus_months(14),
            PartitionMonth {
                year: 2028,
                month: 1
            }
        );
        assert_eq!(nov.plus_months(0), nov);
    }

    #[test]
    fn suffix_and_bounds_are_fixed_width() {
        let m = PartitionMonth {
            year: 2026,
            month: 5,
        };
        assert_eq!(m.suffix(), "2026_05");
        assert_eq!(m.lower(), "2026-05-01");
        assert_eq!(m.upper(), "2026-06-01");
        let dec = PartitionMonth {
            year: 2026,
            month: 12,
        };
        assert_eq!(dec.upper(), "2027-01-01");
    }

    #[test]
    fn months_to_create_spans_current_through_buffer_inclusive() {
        let months = months_to_create(at(2026, 6, 5), 3);
        assert_eq!(
            months,
            vec![
                PartitionMonth {
                    year: 2026,
                    month: 6
                },
                PartitionMonth {
                    year: 2026,
                    month: 7
                },
                PartitionMonth {
                    year: 2026,
                    month: 8
                },
                PartitionMonth {
                    year: 2026,
                    month: 9
                },
            ],
        );
    }

    #[test]
    fn create_ddl_is_if_not_exists_with_bounds() {
        let t = RETENTION[0];
        let sql = create_ddl(
            &t,
            PartitionMonth {
                year: 2026,
                month: 5,
            },
        );
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS audit.events_2026_05"));
        assert!(sql.contains("PARTITION OF audit.events"));
        assert!(sql.contains("FROM ('2026-05-01') TO ('2026-06-01')"));
    }

    /// A child partition governs its own direct-access reads, so `create_ddl`
    /// alone leaves a cross-tenant hole. Pins that the hardening targets the
    /// child, carries the predicate both ways, and is re-runnable.
    #[test]
    fn harden_child_ddl_isolates_the_child_and_is_rerunnable() {
        let t = RETENTION[0];
        let sql = harden_child_ddl(
            &t,
            PartitionMonth {
                year: 2026,
                month: 5,
            },
        );
        assert!(sql.contains("ALTER TABLE audit.events_2026_05 ENABLE ROW LEVEL SECURITY"));
        assert!(sql.contains("ALTER TABLE audit.events_2026_05 FORCE ROW LEVEL SECURITY"));
        assert!(
            !sql.contains("ON audit.events '"),
            "must not touch the parent"
        );
        assert!(sql.contains("CREATE POLICY tenant_isolation ON audit.events_2026_05"));
        assert!(sql.contains("CREATE POLICY maintenance_access ON audit.events_2026_05"));
        assert_eq!(
            sql.matches("organization_id = current_setting(''app.organization_id'', true)")
                .count(),
            2,
            "tenant_isolation needs the parent's organization predicate on USING *and* WITH CHECK"
        );
        assert_eq!(sql.matches("IF NOT EXISTS (").count(), 3);
        assert!(
            sql.contains("c.relrowsecurity AND c.relforcerowsecurity"),
            "the ALTERs must be skipped on a partition already hardened"
        );
        assert!(sql.contains("p.polname = 'tenant_isolation'"));
        assert!(sql.contains("p.polname = 'maintenance_access'"));
        assert!(
            sql.contains(
                "TO auth_maintenance, notifications_maintenance \
                 USING (current_user IN (''auth_maintenance'', ''notifications_maintenance''))"
            ),
            "the child admits exactly the parent's two lanes: {sql}"
        );
    }

    /// The defect `tenant_key` exists to prevent: a project-keyed table has no
    /// `organization_id` column, so an organization predicate would refuse
    /// every direct-child query.
    #[test]
    fn a_project_keyed_table_is_hardened_with_the_project_guc_not_the_organization_one() {
        let t = PartitionedTable {
            schema: "runs",
            table: "run_events",
            retention_days: 90,
            drop_enabled: true,
            tenant_key: "project_id",
            lanes: &[MaintenanceLane::Notifications],
        };
        let sql = harden_child_ddl(
            &t,
            PartitionMonth {
                year: 2026,
                month: 8,
            },
        );
        assert!(
            sql.contains("project_id = current_setting(''app.project_id'', true)"),
            "{sql}"
        );
        assert!(
            !sql.contains("organization_id"),
            "no trace of the wrong key: {sql}"
        );
    }

    #[test]
    fn the_registry_names_a_real_key_for_every_table() {
        for t in RETENTION {
            assert!(
                matches!(t.tenant_key, "organization_id" | "project_id"),
                "{}.{} names unknown tenant_key {}",
                t.schema,
                t.table,
                t.tenant_key
            );
            assert!(
                !t.lanes.is_empty(),
                "{}.{} admits no lane — its children would render an empty `TO`",
                t.schema,
                t.table
            );
        }
    }

    fn short_window() -> PartitionedTable {
        PartitionedTable {
            schema: "s",
            table: "t",
            retention_days: 90,
            drop_enabled: true,
            tenant_key: "project_id",
            lanes: &[MaintenanceLane::Notifications],
        }
    }

    #[test]
    fn is_expired_respects_the_window_at_the_upper_bound() {
        let t = short_window();
        let march = PartitionMonth {
            year: 2026,
            month: 3,
        };
        assert!(is_expired(&t, march, at(2026, 7, 5)));
        let july = PartitionMonth {
            year: 2026,
            month: 7,
        };
        assert!(!is_expired(&t, july, at(2026, 7, 5)));
        let may = PartitionMonth {
            year: 2026,
            month: 5,
        };
        assert!(!is_expired(&t, may, at(2026, 7, 5)));
    }

    #[test]
    fn audit_two_year_window_keeps_recent_partitions() {
        let audit = RETENTION.iter().find(|t| t.table == "events").unwrap();
        let recent = PartitionMonth {
            year: 2025,
            month: 6,
        };
        assert!(!is_expired(audit, recent, at(2026, 7, 5)));
        let old = PartitionMonth {
            year: 2023,
            month: 6,
        };
        assert!(is_expired(audit, old, at(2026, 7, 5)));
    }

    #[test]
    fn parse_child_round_trips_and_rejects_junk() {
        assert_eq!(
            parse_child("events", "events_2026_05"),
            Some(PartitionMonth {
                year: 2026,
                month: 5
            }),
        );
        assert_eq!(
            parse_child("usage_events", "usage_events_2027_01"),
            Some(PartitionMonth {
                year: 2027,
                month: 1
            }),
        );
        assert_eq!(parse_child("events", "events"), None);
        assert_eq!(parse_child("events", "events_backup"), None);
        assert_eq!(parse_child("events", "events_2026_13"), None);
        assert_eq!(parse_child("events", "events_2026_00"), None);
        assert_eq!(parse_child("events", "events_26_5"), None);
        assert_eq!(parse_child("events", "other_2026_05"), None);
    }

    #[test]
    fn drop_ddl_targets_the_child_partition() {
        let t = short_window();
        assert_eq!(
            drop_ddl(
                &t,
                PartitionMonth {
                    year: 2026,
                    month: 1
                }
            ),
            "DROP TABLE IF EXISTS s.t_2026_01",
        );
    }
}
