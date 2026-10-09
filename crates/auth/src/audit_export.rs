//! The audit chain's exports, built in the background. An owner or admin asks
//! for a range and a format (`POST /internal/audit/organizations/{id}/exports`);
//! the request queues the export and starts its build in a task of its own,
//! and [`sweep`], on a five-minute timer inside `telmoni serve`, takes up any
//! build a restart cut short. A finished file waits for its requester alone
//! for [`KEEP_FOR_SECS`], then the sweep deletes it; it is recorded on the
//! chain as an export when it is built, since the chain's rows leaving the
//! building is what a leak investigation must find.
//!
//! Every query runs with the organization and the requester bound: the
//! table's policy admits an export to nobody else.

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Semaphore;
use uuid::Uuid;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{
    PersonAndOrganization, Scoped, maintenance_scope, organization_scope,
};
use telmoni_shared::{AuditAction, OrganizationId, TelmoniError, TelmoniResourceKind, UserId};

use crate::AppState;
use crate::db::AuthLane;
use crate::db::audit::{self, ExportRow};
use crate::db::audit_exports::{self, Claimed};
use crate::db::organization_members;

/// How long a finished file is kept for its requester: a week to come back
/// for it, and no longer, since it is the chain's rows again. ⚠ **Published**
/// — the docs' audit log page, the console's export dialog and the hosted
/// privacy policy all say 7 days — so a change here is a change to each.
pub const KEEP_FOR_SECS: i32 = 7 * 24 * 60 * 60;

/// The most rows one file holds. Past it the export fails as `too_large`
/// rather than stopping short: a file missing the end of its range would
/// still verify, and say less than it seems to.
pub const MAX_ROWS: i64 = 25_000;

/// How many exports one person may have building in one organization at once.
pub const MAX_UNFINISHED: i64 = 3;

/// How many of one person's exports one organization keeps: a new one deletes
/// their oldest finished one past this, so files cannot pile up on the
/// database however often somebody exports.
pub const MAX_KEPT: i64 = 10;

/// The guard behind [`MAX_ROWS`] for rows with large details. A file is
/// assembled in memory, then copied once more to be stored, inside a server
/// that also runs every other module: this keeps two builds well inside it.
const MAX_BYTES: usize = 24 * 1024 * 1024;

/// Rows read per query while a file is built.
const PAGE: i64 = 5_000;

/// Builds an export gets before it is given up as `error`.
const MAX_ATTEMPTS: i32 = 3;

/// How long a build holds its export before another may take it over: far
/// past a build of [`MAX_ROWS`].
const LEASE_SECS: i32 = 10 * 60;

/// How long a queued export waits for the build its request started before
/// [`sweep`] takes it: past any request's own life.
const SETTLE_SECS: i32 = 30;

/// The most exports one [`sweep`] builds again; the rest wait for the next.
const RETRY_BATCH: i64 = 20;

/// Builds at once in this process, each holding its file in memory. One
/// waiting here is still `queued`, so another replica's [`sweep`] may take it.
static BUILDS: Semaphore = Semaphore::const_new(2);

/// The two shapes a file comes in: JSON, which the docs' script verifies, and
/// CSV, for a spreadsheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    /// One JSON document, the rows under `events`.
    Json,
    /// One header row, then one line a row, `metadata` as its JSON.
    Csv,
}

impl ExportFormat {
    /// The column's spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Csv => "csv",
        }
    }

    /// The format a column names, `None` for a spelling it never writes.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "json" => Some(Self::Json),
            "csv" => Some(Self::Csv),
            _ => None,
        }
    }

    /// What a download says the file is.
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Json => "application/json; charset=utf-8",
            Self::Csv => "text/csv; charset=utf-8",
        }
    }
}

/// What one build came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Built {
    /// The file is stored and the export recorded on the chain.
    Ready,
    /// More than one file holds; failed for good as `too_large`.
    TooLarge,
    /// Another build holds it, or it was finished already.
    NotClaimed,
    /// Its requester no longer administers the organization: an export is
    /// theirs alone and only while they may read the log, so it is dropped,
    /// recorded nowhere, rather than built for nobody.
    Withdrawn,
}

/// Build a just-queued export in a task of its own, so its request answers at
/// once. A failure here is [`sweep`]'s to take up.
pub fn spawn_build(
    state: Arc<AppState>,
    id: Uuid,
    organization_id: OrganizationId,
    user_id: UserId,
) {
    tokio::spawn(async move {
        if let Err(e) = build(&state, id, &organization_id, &user_id).await {
            tracing::warn!(
                export_id = %id,
                organization_id = %organization_id,
                error = %e,
                "audit export build failed; the exports sweep retries it while attempts remain"
            );
        }
    });
}

/// What one exports sweep did: counts only, no ids.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExportsSwept {
    /// Exports a restart had cut short, finished now.
    pub finished: u64,
    /// Exports past [`KEEP_FOR_SECS`], deleted with their files.
    pub expired: u64,
}

/// The exports' own sweep: every export past its keep deleted, file and all,
/// then every one queued for longer than its request could have taken, or
/// running past its lease, built again — or given up as `error` once its
/// attempts are spent. The retry net behind the build each request starts.
pub async fn sweep(state: &AppState) -> Result<ExportsSwept, TelmoniError> {
    // Apart, so an expiry that fails — a backlog past the statement timeout —
    // still leaves the builds retried, the person a stuck one holds at its
    // limit freed, and the failure answered once they are.
    let expired = expire(state).await;

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let due = audit_exports::unfinished_past_due(&mut tx, SETTLE_SECS, RETRY_BATCH).await?;
    tx.commit().await?;

    let mut finished = 0u64;
    for due in due {
        match build(state, due.id, &due.organization_id, &due.user_id).await {
            Ok(Built::Ready | Built::TooLarge) => finished += 1,
            Ok(Built::NotClaimed | Built::Withdrawn) => {}
            Err(e) => tracing::warn!(
                export_id = %due.id,
                organization_id = %due.organization_id,
                error = %e,
                "audit export build failed; a later sweep retries it while attempts remain"
            ),
        }
    }
    Ok(ExportsSwept {
        finished,
        expired: expired?,
    })
}

/// Every export past its keep deleted, file and all.
async fn expire(state: &AppState) -> Result<u64, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let expired = audit_exports::delete_expired(&mut tx).await?;
    tx.commit().await?;
    Ok(expired)
}

/// Whether the requester may still read the organization's whole audit log:
/// an export is theirs alone, and only while they do.
async fn still_exporter(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    organization_id: &OrganizationId,
    user_id: &UserId,
) -> sqlx::Result<bool> {
    Ok(
        organization_members::role_on_organization(tx, organization_id, user_id)
            .await?
            .is_some_and(|role| role.can_view_rolled_up_audit()),
    )
}

/// The organization's scope with the requester bound, which is the only one
/// the table's policy shows an export to.
async fn scope<'a>(
    state: &'a AppState,
    organization_id: &OrganizationId,
    user_id: &UserId,
) -> sqlx::Result<Scoped<'a, PersonAndOrganization>> {
    organization_scope(&state.db, organization_id)
        .await?
        .bind_person(user_id)
        .await
}

/// Build one export: claim it, read its stretch of the chain a page at a
/// time, store the file and record it on the chain. A build that fails hands
/// the export back to the queue, or fails it for good on its last attempt.
pub async fn build(
    state: &AppState,
    id: Uuid,
    organization_id: &OrganizationId,
    user_id: &UserId,
) -> Result<Built, TelmoniError> {
    let _permit = BUILDS
        .acquire()
        .await
        .map_err(|_| TelmoniError::Internal("the audit export builds were shut".into()))?;

    let mut tx = scope(state, organization_id, user_id).await?;
    if !still_exporter(&mut tx, organization_id, user_id).await? {
        audit_exports::withdraw(&mut tx, id, organization_id, user_id).await?;
        tx.commit().await?;
        return Ok(Built::Withdrawn);
    }
    let claimed = audit_exports::claim(
        &mut tx,
        id,
        organization_id,
        user_id,
        MAX_ATTEMPTS,
        LEASE_SECS,
    )
    .await?;
    if claimed.is_none() {
        audit_exports::fail_spent(
            &mut tx,
            id,
            organization_id,
            user_id,
            MAX_ATTEMPTS,
            KEEP_FOR_SECS,
        )
        .await?;
    }
    tx.commit().await?;
    let Some(claimed) = claimed else {
        return Ok(Built::NotClaimed);
    };

    match write_and_store(state, id, organization_id, user_id, &claimed).await {
        Ok(built) => Ok(built),
        Err(e) => {
            // The build's own error is the one answered: a hand-back that
            // fails too leaves the lease to run out, and the sweep after it
            // takes the export up as it takes up one a crash dropped.
            if let Err(hand_back) = hand_back(state, id, organization_id, user_id, &claimed).await {
                tracing::warn!(
                    export_id = %id,
                    organization_id = %organization_id,
                    error = %hand_back,
                    "audit export not handed back; its lease runs out instead"
                );
            }
            Err(e)
        }
    }
}

/// An export whose build failed: back to the queue, or failed for good as
/// `error` on its last attempt.
async fn hand_back(
    state: &AppState,
    id: Uuid,
    organization_id: &OrganizationId,
    user_id: &UserId,
    claimed: &Claimed,
) -> Result<(), TelmoniError> {
    let mut tx = scope(state, organization_id, user_id).await?;
    if claimed.attempts >= MAX_ATTEMPTS {
        audit_exports::finish_failed(
            &mut tx,
            id,
            organization_id,
            user_id,
            claimed.attempts,
            "error",
            KEEP_FOR_SECS,
        )
        .await?;
    } else {
        audit_exports::release(&mut tx, id, organization_id, user_id, claimed.attempts).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The build proper, once claimed.
async fn write_and_store(
    state: &AppState,
    id: Uuid,
    organization_id: &OrganizationId,
    user_id: &UserId,
    claimed: &Claimed,
) -> Result<Built, TelmoniError> {
    let format = ExportFormat::parse(&claimed.format)
        .ok_or_else(|| TelmoniError::Internal(format!("export {id} names no known format")))?;

    // Read-only, and every row in the stretch is final: the chain's lock
    // hands out `seq` and is held to commit, so a row is visible only once
    // every row before it is.
    let mut tx = scope(state, organization_id, user_id).await?;
    let bounds = audit::seq_bounds(
        &mut tx,
        organization_id,
        claimed.range_from,
        claimed.range_to,
    )
    .await?;
    let mut file = FileWriter::new(format, organization_id, claimed)?;
    let mut too_large = false;
    if let Some((first, last)) = bounds {
        if last - first + 1 > MAX_ROWS {
            too_large = true;
        } else {
            let mut after = first - 1;
            loop {
                let page = audit::export_page(&mut tx, organization_id, after, last, PAGE).await?;
                let Some(tail) = page.last() else { break };
                after = tail.seq;
                for row in &page {
                    file.row(row)?;
                }
                if file.len() > MAX_BYTES {
                    too_large = true;
                    break;
                }
            }
        }
    }
    tx.commit().await?;

    let mut tx = scope(state, organization_id, user_id).await?;
    if too_large {
        audit_exports::finish_failed(
            &mut tx,
            id,
            organization_id,
            user_id,
            claimed.attempts,
            "too_large",
            KEEP_FOR_SECS,
        )
        .await?;
        tx.commit().await?;
        return Ok(Built::TooLarge);
    }
    // Again at the end, in the transaction that records it: a role lost while
    // the file was written is the same as one lost before it was claimed.
    if !still_exporter(&mut tx, organization_id, user_id).await? {
        audit_exports::withdraw(&mut tx, id, organization_id, user_id).await?;
        tx.commit().await?;
        return Ok(Built::Withdrawn);
    }
    let row_count = file.rows;
    let bytes = file.finish();
    if !audit_exports::finish_ready(
        &mut tx,
        id,
        organization_id,
        user_id,
        claimed.attempts,
        &bytes,
        row_count,
        KEEP_FOR_SECS,
    )
    .await?
    {
        // No longer this attempt's: its lease ran out and another build took
        // it over, which records it, or it is gone — given up as spent, or
        // deleted with its requester or its organization.
        return Ok(Built::NotClaimed);
    }
    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id,
            in_project: None,
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Exported,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "export": "audit_log",
                "format": format.as_str(),
                "from": claimed.range_from,
                "to": claimed.range_to,
                "rows": row_count,
            })),
        },
    )
    .await?;
    // And once more with the chain's lock held, which a change of role takes
    // too, to write its own record: a demotion that landed after the read
    // above is seen here, so no record stands for an export its requester
    // could no longer start.
    if !still_exporter(&mut tx, organization_id, user_id).await? {
        tx.rollback().await?;
        let mut tx = scope(state, organization_id, user_id).await?;
        audit_exports::withdraw(&mut tx, id, organization_id, user_id).await?;
        tx.commit().await?;
        return Ok(Built::Withdrawn);
    }
    tx.commit().await?;
    Ok(Built::Ready)
}

/// The columns of a CSV file, a JSON row's fields in its order.
const CSV_HEADER: &str = "seq,id,created_at,actor_id,action,resource_kind,resource_id,\
                          in_project,metadata,prev_hash,row_hash\r\n";

/// A file, written as its rows are read: JSON's envelope around them, or a
/// CSV with one header row.
struct FileWriter {
    format: ExportFormat,
    out: Vec<u8>,
    rows: i64,
}

impl FileWriter {
    fn new(
        format: ExportFormat,
        organization_id: &OrganizationId,
        claimed: &Claimed,
    ) -> Result<Self, TelmoniError> {
        let mut out = Vec::new();
        match format {
            // The envelope's fields, then `events` left open for the rows;
            // `finish` closes both.
            ExportFormat::Json => {
                out.extend_from_slice(b"{\"organization_id\":");
                serde_json::to_writer(&mut out, organization_id.as_str()).map_err(unwritable)?;
                out.extend_from_slice(b",\"exported_at\":");
                serde_json::to_writer(&mut out, &Utc::now()).map_err(unwritable)?;
                out.extend_from_slice(b",\"from\":");
                serde_json::to_writer(&mut out, &claimed.range_from).map_err(unwritable)?;
                out.extend_from_slice(b",\"to\":");
                serde_json::to_writer(&mut out, &claimed.range_to).map_err(unwritable)?;
                out.extend_from_slice(b",\"events\":[");
            }
            // The byte-order mark is how a spreadsheet knows the file is
            // UTF-8: without it, Excel reads a name with an accent as two
            // wrong characters.
            ExportFormat::Csv => {
                out.extend_from_slice("\u{feff}".as_bytes());
                out.extend_from_slice(CSV_HEADER.as_bytes());
            }
        }
        Ok(Self {
            format,
            out,
            rows: 0,
        })
    }

    fn row(&mut self, row: &ExportRow) -> Result<(), TelmoniError> {
        match self.format {
            ExportFormat::Json => {
                if self.rows > 0 {
                    self.out.push(b',');
                }
                serde_json::to_writer(&mut self.out, row).map_err(unwritable)?;
            }
            ExportFormat::Csv => {
                let metadata = match &row.metadata {
                    Some(m) => serde_json::to_string(m).map_err(unwritable)?,
                    None => String::new(),
                };
                let cells = [
                    row.seq.to_string(),
                    row.id.to_string(),
                    row.created_at.clone(),
                    row.actor_id.clone(),
                    row.action.clone(),
                    row.resource_kind.clone(),
                    row.resource_id.clone().unwrap_or_default(),
                    row.in_project.clone().unwrap_or_default(),
                    metadata,
                    row.prev_hash.clone().unwrap_or_default(),
                    row.row_hash.clone(),
                ];
                for (i, cell) in cells.iter().enumerate() {
                    if i > 0 {
                        self.out.push(b',');
                    }
                    csv_cell(&mut self.out, cell);
                }
                self.out.extend_from_slice(b"\r\n");
            }
        }
        self.rows += 1;
        Ok(())
    }

    fn len(&self) -> usize {
        self.out.len()
    }

    fn finish(mut self) -> Vec<u8> {
        if self.format == ExportFormat::Json {
            self.out.extend_from_slice(b"]}");
        }
        self.out
    }
}

/// One CSV cell, as RFC 4180 has it: quoted when it holds a comma, a quote or
/// a line break, its quotes doubled. And defused, as OWASP advises: a cell a
/// spreadsheet would run as a formula — one starting with `=`, `+`, `-`, `@`,
/// a tab or a carriage return — starts with `'` inside its quotes instead,
/// since an event's details carry names people typed, and the file opens in
/// somebody's spreadsheet. The JSON file is the one that verifies.
fn csv_cell(out: &mut Vec<u8>, cell: &str) {
    let defuse = cell.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let quote = defuse || cell.contains([',', '"', '\n', '\r']);
    if quote {
        out.push(b'"');
    }
    if defuse {
        out.push(b'\'');
    }
    out.extend_from_slice(cell.replace('"', "\"\"").as_bytes());
    if quote {
        out.push(b'"');
    }
}

/// Writing into memory has no I/O to fail; this is serde's own refusal.
fn unwritable(e: serde_json::Error) -> TelmoniError {
    TelmoniError::Internal(format!("an audit export could not be written: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(value: &str) -> String {
        let mut out = Vec::new();
        csv_cell(&mut out, value);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn a_plain_cell_is_written_as_it_is() {
        assert_eq!(cell("created"), "created");
        assert_eq!(cell(""), "");
    }

    #[test]
    fn a_cell_with_a_comma_a_quote_or_a_line_break_is_quoted() {
        assert_eq!(cell("a,b"), "\"a,b\"");
        assert_eq!(cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(cell("two\nlines"), "\"two\nlines\"");
        assert_eq!(
            cell("{\"name\":\"Ada\",\"role\":\"admin\"}"),
            "\"{\"\"name\"\":\"\"Ada\"\",\"\"role\"\":\"\"admin\"\"}\""
        );
    }

    #[test]
    fn a_cell_a_spreadsheet_would_run_is_defused() {
        assert_eq!(cell("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        assert_eq!(cell("+1"), "\"'+1\"");
        assert_eq!(cell("-1"), "\"'-1\"");
        assert_eq!(cell("@SUM(A1)"), "\"'@SUM(A1)\"");
        assert_eq!(cell("\tx"), "\"'\tx\"");
    }

    fn row(seq: i64, metadata: Option<serde_json::Value>) -> ExportRow {
        ExportRow {
            seq,
            id: Uuid::nil(),
            created_at: "2026-10-08T19:24:59.123456Z".into(),
            actor_id: "user_a".into(),
            action: "created".into(),
            resource_kind: "project".into(),
            resource_id: Some("project_a".into()),
            in_project: None,
            metadata,
            prev_hash: None,
            row_hash: "ab".into(),
        }
    }

    fn claimed() -> Claimed {
        Claimed {
            format: "json".into(),
            range_from: None,
            range_to: Utc::now(),
            attempts: 1,
        }
    }

    #[test]
    fn a_json_file_is_one_document_with_its_rows_in_order() {
        let organization = OrganizationId::new();
        let mut file = FileWriter::new(ExportFormat::Json, &organization, &claimed()).unwrap();
        file.row(&row(1, None)).unwrap();
        file.row(&row(2, Some(json!({ "name": "Ada" })))).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&file.finish()).unwrap();
        assert_eq!(parsed["organization_id"], json!(organization.as_str()));
        assert_eq!(parsed["from"], json!(null));
        let events = parsed["events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1]["seq"], json!(2));
        assert_eq!(events[1]["metadata"]["name"], json!("Ada"));
    }

    #[test]
    fn an_empty_range_is_a_json_file_with_no_events() {
        let organization = OrganizationId::new();
        let file = FileWriter::new(ExportFormat::Json, &organization, &claimed()).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&file.finish()).unwrap();
        assert_eq!(parsed["events"], json!([]));
    }

    #[test]
    fn a_csv_file_has_its_header_and_one_line_a_row() {
        let organization = OrganizationId::new();
        let mut file = FileWriter::new(ExportFormat::Csv, &organization, &claimed()).unwrap();
        file.row(&row(1, Some(json!({ "name": "Ada" })))).unwrap();
        let text = String::from_utf8(file.finish()).unwrap();
        let mut lines = text.trim_start_matches('\u{feff}').split("\r\n");
        assert_eq!(lines.next().unwrap(), CSV_HEADER.trim_end());
        assert_eq!(
            lines.next().unwrap(),
            "1,00000000-0000-0000-0000-000000000000,2026-10-08T19:24:59.123456Z,user_a,\
             created,project,project_a,,\"{\"\"name\"\":\"\"Ada\"\"}\",,ab"
        );
        assert_eq!(lines.next(), Some(""));
    }
}
