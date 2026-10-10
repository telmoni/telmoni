//! The ClickHouse file, and how `telmoni migrate` applies it: after the
//! Postgres sets, as the migrator's user, one statement per request, since
//! sqlx's migrator speaks only Postgres and ClickHouse's HTTP interface takes
//! one statement at a time. Every statement is `IF NOT EXISTS`, so every run
//! applies the whole file, and a run over an applied store changes nothing.
//!
//! ⚠ **An edited file refuses to run over a store an older one made**, as an
//! edited Postgres migration does over its database. Before launch the file
//! is edited in place, and `IF NOT EXISTS` would skip the edit in silence: a
//! policy tightened that never landed, a column added that every insert then
//! names. So the store keeps the digest of the statements it is made from
//! (`telemetry.migrations`), and a run whose statements differ stops before
//! any of them but the ledger's own. The digest is written as soon as the
//! ledger exists, before the rest of the file runs, so a run that stops
//! partway still marks the store with the file it was being made from; and a
//! new ledger over tables already there, which no run of this kind leaves,
//! refuses too. Comments are not statements, so editing one changes nothing.

use anyhow::Context;
use sha2::{Digest, Sha256};

use crate::store::within;

/// The one ClickHouse migration, embedded so it cannot drift from the binary
/// that applies it.
pub const MIGRATION: &str = include_str!("../clickhouse/20261001100000_telemetry_initial.sql");

/// How many of the file's statements make the ledger, its first: the
/// database and the `migrations` table. They run before the ledger is read.
const LEDGER_STATEMENTS: usize = 2;

const LEDGER_DIGESTS: &str = "SELECT DISTINCT digest FROM telemetry.migrations";

const RECORD_DIGEST: &str = "INSERT INTO telemetry.migrations (digest) VALUES ({digest:String})";

/// Whether the spans' table is there already: it comes after the ledger, so
/// a new ledger finds none, unless the store was made before it had one.
const SPANS_EXIST: &str = "EXISTS TABLE telemetry.spans";

/// The longest the migrator waits on one statement, here and in the purge.
/// Each is a definition, a ledger read, a listing or a partition's drop,
/// which a healthy server answers at once; a server that stalls would
/// otherwise hold `migrate` to the chart's deadline and a host's nightly
/// `rotate` for good.
pub(crate) const STATEMENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Apply every statement of [`MIGRATION`], in order, as `client`'s user —
/// the one `tenant_isolation` exempts, whose own `migrator_reads_all` admits
/// it to every row, and whom the view runs as — unless the store was made
/// from other statements. Answers how many statements ran.
pub async fn apply(client: &clickhouse::Client) -> anyhow::Result<usize> {
    let statements = statements(MIGRATION);
    let digest = digest(&statements);
    let count = statements.len();
    let (ledger, rest) = statements.split_at(LEDGER_STATEMENTS.min(count));
    run(client, ledger, 0, count).await?;
    match recorded_digest(client)
        .await
        .context("reading the ClickHouse store's ledger")?
    {
        Some(recorded) if recorded != digest => anyhow::bail!(
            "the ClickHouse store was made from other statements than this binary's file \
             (its ledger holds {recorded}, the file is {digest}); before launch the file is \
             edited in place, so the store is rebuilt: `make db-reset` on a laptop"
        ),
        Some(_) => {}
        None => {
            // An empty ledger over tables already made — a store from before
            // the ledger, or one whose ledger was emptied — would take this
            // file's digest over tables IF NOT EXISTS then leaves as they are.
            let spans: u8 = within(STATEMENT_TIMEOUT, client.query_raw(SPANS_EXIST).fetch_one())
                .await
                .context("reading whether the ClickHouse store has its tables")?;
            if spans != 0 {
                anyhow::bail!(
                    "the ClickHouse store has its tables but no record of the file that made \
                     them; before launch the file is edited in place, so the store is rebuilt: \
                     `make db-reset` on a laptop"
                );
            }
            within(
                STATEMENT_TIMEOUT,
                client
                    .query_raw(RECORD_DIGEST)
                    .param("digest", &digest)
                    .execute(),
            )
            .await
            .context("recording the ClickHouse file's digest")?;
        }
    }
    run(client, rest, ledger.len(), count).await?;
    Ok(count)
}

/// Run `statements`, the file's from number `first + 1` of `count` on.
async fn run(
    client: &clickhouse::Client,
    statements: &[String],
    first: usize,
    count: usize,
) -> anyhow::Result<()> {
    for (index, statement) in statements.iter().enumerate() {
        within(STATEMENT_TIMEOUT, client.query_raw(statement).execute())
            .await
            .with_context(|| {
                let number = first + index + 1;
                format!("ClickHouse migration statement {number} of {count}")
            })?;
    }
    Ok(())
}

/// The digest of the statements the store is made from, or `None` for a
/// store whose ledger is new. Refuses a ledger holding two: a store made
/// from one file holds one.
async fn recorded_digest(client: &clickhouse::Client) -> anyhow::Result<Option<String>> {
    let mut digests: Vec<String> = within(
        STATEMENT_TIMEOUT,
        client.query_raw(LEDGER_DIGESTS).fetch_all(),
    )
    .await?;
    match digests.len() {
        0 | 1 => Ok(digests.pop()),
        n => {
            anyhow::bail!("the ClickHouse store's ledger holds {n} digests, where a store has one")
        }
    }
}

/// The SHA-256 of the statements, in hex: what the store was made from,
/// comments aside. A comment taken out of a statement leaves its line, or
/// the end of its line, as whitespace, so a statement is hashed as its lines
/// with their trailing whitespace trimmed and the blank ones dropped.
#[must_use]
pub fn digest(statements: &[String]) -> String {
    let mut hash = Sha256::new();
    for statement in statements {
        for line in statement
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
        {
            hash.update(line.as_bytes());
            hash.update(b"\n");
        }
        hash.update(b";\n");
    }
    format!("{:x}", hash.finalize())
}

/// The statements of a file of them, in order: `--` comments removed, each
/// ended by a `;` outside a quoted string or identifier, the blank ones
/// dropped. The file uses no `/* */` comment.
#[must_use]
pub fn statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(open) = quote {
            current.push(c);
            if c == '\\' {
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                }
            } else if c == open {
                // A doubled quote closes here and opens again on the next.
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => {
                quote = Some(c);
                current.push(c);
            }
            '-' if chars.peek() == Some(&'-') => {
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        current.push('\n');
                        break;
                    }
                }
            }
            ';' => finish(&mut statements, &mut current),
            _ => current.push(c),
        }
    }
    finish(&mut statements, &mut current);
    statements
}

fn finish(statements: &mut Vec<String>, current: &mut String) {
    let statement = current.trim();
    if !statement.is_empty() {
        statements.push(statement.to_owned());
    }
    current.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_semicolon_ends_a_statement_only_outside_quotes_and_comments() {
        let sql = "-- a comment; with a semicolon and an apostrophe's\n\
                   CREATE TABLE IF NOT EXISTS t (a String DEFAULT 'x;y') ENGINE = Memory;\n\
                   SELECT 'it''s; fine', `odd;name`, \"also;odd\" -- trailing; comment\n\
                   ;\n\n;  ;\n\
                   SELECT 'back\\'slash;'";
        assert_eq!(
            statements(sql),
            [
                "CREATE TABLE IF NOT EXISTS t (a String DEFAULT 'x;y') ENGINE = Memory",
                "SELECT 'it''s; fine', `odd;name`, \"also;odd\"",
                "SELECT 'back\\'slash;'",
            ]
        );
    }

    /// What a store keeps of its file is its statements: a comment edited
    /// changes nothing, a statement edited is another file.
    #[test]
    fn the_digest_is_the_statements_comments_aside() {
        let file = digest(&statements(MIGRATION));
        assert_eq!(file.len(), 64);
        let commented = format!("-- a new comment; with a semicolon\n{MIGRATION}");
        assert_eq!(digest(&statements(&commented)), file);
        let inside = MIGRATION.replacen(
            "    units                    UInt32,",
            "    -- a note on a line of its own, inside a statement\n    \
             units                    UInt32, -- and one after a column",
            1,
        );
        assert_ne!(inside, MIGRATION, "the edit found nothing to change");
        assert_eq!(
            digest(&statements(&inside)),
            file,
            "a comment inside a statement changed what the store was made from"
        );
        let edited = MIGRATION.replacen("ORDER BY applied_at", "ORDER BY digest", 1);
        assert_ne!(edited, MIGRATION, "the edit found nothing to change");
        assert_ne!(digest(&statements(&edited)), file);
    }

    /// The ledger is read before anything after it runs, so it comes first.
    #[test]
    fn the_file_makes_its_ledger_first() {
        let statements = statements(MIGRATION);
        assert!(statements.len() > LEDGER_STATEMENTS);
        assert!(statements[0].starts_with("CREATE DATABASE IF NOT EXISTS telemetry "));
        assert!(statements[1].starts_with("CREATE TABLE IF NOT EXISTS telemetry.migrations"));
    }

    #[test]
    fn a_file_of_comments_is_no_statements() {
        assert!(statements("-- nothing\n\n-- here;\n").is_empty());
        assert!(statements("").is_empty());
    }
}
