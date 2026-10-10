//! The one query module: every read and write the module's own user makes of
//! ClickHouse goes through here. The migrator's are `schema`'s and
//! `retention`'s.
//!
//! ⚠ **A read names its projects twice; an insert names none.** The row
//! policy `tenant_isolation` admits only the rows of the projects a query's
//! setting [`PROJECTS_SETTING`] names, and fails a read that names none
//! rather than answer every project's, as the Postgres policies read
//! `app.project_id`. It holds every user but the one that made the tables,
//! so each read also names its projects in its own `WHERE`
//! ([`PROJECTS_PARAM`]), which still holds where one user both made the
//! tables and reads them — the Compose quickstart — as a query naming its
//! tenant holds where row-level security does not. [`Store::read`] binds
//! both from one [`Projects`], which cannot be empty, and a read whose SQL
//! names no projects, or that binds them twice, is never sent. An insert
//! carries neither: ClickHouse gathers into one asynchronous insert only the
//! inserts whose settings match, and a policy filters reads alone.
//!
//! Every value reaches ClickHouse as a server-side parameter (`{name:Type}`
//! in the SQL, [`Read::param`] in the code), never spliced into the
//! statement, and a read's SQL is a literal, so nothing can be.

use std::time::Duration;

use chrono::{DateTime, Utc};
use clickhouse::{Row, RowOwned, RowRead};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::{ProjectId, Redacted, SpanKind, SpanStatus};

/// The ClickHouse database telemetry's tables live in.
pub const DATABASE: &str = "telemetry";

/// The per-query setting the row policy reads: the projects a read may see,
/// as a JSON array of their ids, so no id's characters can name another.
/// ClickHouse takes a setting of its own only under a prefix its server
/// allows, `SQL_` unless `custom_settings_prefixes` says otherwise.
pub const PROJECTS_SETTING: &str = "SQL_telmoni_projects";

/// How a read names its projects in its own `WHERE`, as every read does:
/// `project_id IN {projects:Array(String)}`, bound by [`Store::read`] from
/// the list the row policy reads.
pub const PROJECTS_PARAM: &str = "{projects:Array(String)}";

/// How long an insert may take to start — the client reads the table's
/// columns first, once per process, over a connection it may still be
/// opening — and to send each chunk of its rows; and how long ClickHouse may
/// take to say it holds them, since an asynchronous insert waits for its
/// flush.
const INSERT_SEND_TIMEOUT: Duration = Duration::from_secs(10);
const INSERT_END_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest a read may run on the server, in seconds: one node answers
/// every project, so a read that runs away is every reader's outage. The
/// console waits ten seconds for any answer, so the server stops a read
/// short of that and the console hears why.
const READ_MAX_EXECUTION_SECS: &str = "8";

/// The longest the client waits for a read: the console's own wait, past
/// which no one is waiting for the answer. A server or a network that never
/// answers is given up on too.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// A client for the ClickHouse `url` names, as the user and password it
/// carries, as a DSN does (`http://user:password@host:8123`). The client
/// sends them as headers, never in a URL, so they are taken out of it. A
/// query string is refused: a setting is the code's to choose, never the
/// URL's. In a Kubernetes pod, plain `http` is refused unless it stays in
/// the cluster: a loopback host, or a Service's name with its namespace.
/// Nothing is dialled until the first query.
///
/// ⚠ An error names what is wrong with the URL, never the URL: it holds a
/// password.
pub fn client(url: &str) -> anyhow::Result<clickhouse::Client> {
    let Dsn {
        origin,
        user,
        password,
    } = Dsn::parse(url, telmoni_shared::envelope::on_deployed_tier())?;
    let mut client = clickhouse::Client::default().with_url(origin);
    if !user.is_empty() {
        client = client.with_user(user);
    }
    if let Some(password) = password {
        client = client.with_password(password.expose());
    }
    Ok(client)
}

/// A ClickHouse URL taken apart: where it is, and who to be there.
struct Dsn {
    /// The URL with no user, no password and no query in it.
    origin: String,
    /// The user, decoded; empty for ClickHouse's `default`.
    user: String,
    /// The password, decoded.
    password: Option<Redacted>,
}

impl Dsn {
    /// `url` taken apart, `deployed` saying whether this is a pod.
    fn parse(url: &str, deployed: bool) -> anyhow::Result<Self> {
        let mut parsed = reqwest::Url::parse(url)
            .map_err(|e| anyhow::anyhow!("the ClickHouse URL does not parse: {e}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            anyhow::bail!("the ClickHouse URL must be http or https: its HTTP interface");
        }
        if parsed.host_str().is_none_or(str::is_empty) {
            anyhow::bail!("the ClickHouse URL names no host");
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            anyhow::bail!("the ClickHouse URL carries a query or a fragment; it names a server");
        }
        if deployed && !encrypted_or_in_cluster(&parsed) {
            anyhow::bail!(
                "the ClickHouse URL is http to a host outside the cluster, in a Kubernetes pod; \
                 its password and every span's ids would cross the network in the clear: use \
                 https, or a Service's name with its namespace, `<service>.<namespace>.svc`"
            );
        }
        let user = percent_decoded(parsed.username())?;
        let password = parsed
            .password()
            .map(percent_decoded)
            .transpose()?
            .map(Redacted::from);
        if parsed.set_username("").is_err() || parsed.set_password(None).is_err() {
            anyhow::bail!("the ClickHouse URL's user and password cannot be taken out of it");
        }
        Ok(Self {
            origin: parsed.into(),
            user,
            password,
        })
    }
}

/// Whether a ClickHouse URL is reached encrypted, or never leaves the
/// cluster: `https`, or `http` to a loopback host or a Service's name with
/// its namespace (`clickhouse.telmoni.svc`, `….svc.cluster.local`). What a
/// pod requires, as it requires TLS of Postgres and of SMTP: an address
/// outside the cluster, a VM in the same network included, is crossed in
/// the clear. Not a bare name: one no Service in the namespace answers falls
/// through the node's DNS search path to whatever outside it does.
fn encrypted_or_in_cluster(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let loopback = matches!(host, "localhost" | "127.0.0.1" | "[::1]");
    let service = host.ends_with(".svc") || host.ends_with(".svc.cluster.local");
    match url.scheme() {
        "https" => true,
        "http" => loopback || service,
        _ => false,
    }
}

/// A URL's user or password as it was before percent-encoding, which a
/// password with a `@`, `:` or `/` in it needs.
fn percent_decoded(encoded: &str) -> anyhow::Result<String> {
    fn hex(digit: u8) -> Option<u8> {
        match digit {
            b'0'..=b'9' => Some(digit - b'0'),
            b'a'..=b'f' => Some(digit - b'a' + 10),
            b'A'..=b'F' => Some(digit - b'A' + 10),
            _ => None,
        }
    }
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut rest = encoded.bytes();
    while let Some(b) = rest.next() {
        if b != b'%' {
            bytes.push(b);
            continue;
        }
        let (Some(high), Some(low)) = (rest.next().and_then(hex), rest.next().and_then(hex)) else {
            anyhow::bail!("the ClickHouse URL has a `%` without two hex digits after it");
        };
        bytes.push(high * 16 + low);
    }
    String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("the ClickHouse URL's user or password is not UTF-8"))
}

/// The projects one read may see: one or more, as the reader's own scope
/// decided them. Never empty, so no read reaches ClickHouse naming none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projects(Vec<ProjectId>);

impl Projects {
    /// One project's rows.
    #[must_use]
    pub fn one(project_id: ProjectId) -> Self {
        Self(vec![project_id])
    }

    /// These projects' rows, or `None` for no project: a read naming none
    /// could see nothing, and is refused here rather than sent.
    #[must_use]
    pub fn of(project_ids: impl IntoIterator<Item = ProjectId>) -> Option<Self> {
        let project_ids: Vec<ProjectId> = project_ids.into_iter().collect();
        (!project_ids.is_empty()).then_some(Self(project_ids))
    }

    /// The ids, as the query's own parameter binds them.
    fn ids(&self) -> Vec<&str> {
        self.0.iter().map(ProjectId::as_str).collect()
    }

    /// The setting's value: the ids as a JSON array.
    fn setting(&self) -> String {
        serde_json::Value::from(self.ids()).to_string()
    }
}

/// One finished span, as `telemetry.spans` holds it: a column for each field
/// of the ingest contract, under the field's own name, and the three the
/// server adds — its project, when it arrived, and its units.
#[derive(Debug, Clone, PartialEq, Row, Serialize, Deserialize)]
pub struct SpanRow {
    /// The project of the key that sent it, and nothing above it.
    pub project_id: ProjectId,
    /// The server's clock when it arrived: its partition's day.
    #[serde(with = "clickhouse::serde::chrono::datetime64::micros")]
    pub received_at: DateTime<Utc>,
    /// What it counts against its plan.
    pub units: u32,
    /// Its run's id, the same on every span of a run.
    #[serde(with = "clickhouse::serde::uuid")]
    pub run_id: Uuid,
    /// Its run's agent: the monitor it is counted under.
    pub agent: String,
    /// Its trace's id, in hex.
    pub trace_id: String,
    /// Its own id, in hex.
    pub span_id: String,
    /// Its parent's id, in hex; empty for a span with none.
    pub parent_span_id: String,
    /// What it is.
    #[serde(with = "text")]
    pub kind: SpanKind,
    /// The agent's name on a run, the tool's on a tool call, its own on the rest.
    pub name: String,
    /// When it started, by the sender's clock.
    #[serde(with = "clickhouse::serde::chrono::datetime64::micros")]
    pub started_at: DateTime<Utc>,
    /// When it ended, by the sender's clock.
    #[serde(with = "clickhouse::serde::chrono::datetime64::micros")]
    pub ended_at: DateTime<Utc>,
    /// How it ended.
    #[serde(with = "text")]
    pub status: SpanStatus,
    /// The exception's class on an error, never its message; empty otherwise.
    pub error_type: String,
    /// The model that answered a model call; empty on every other kind.
    pub model: String,
    /// Its provider, as the conventions spell it; empty on every other kind.
    pub provider: String,
    /// Every input token, the cache's reads and writes among them.
    pub input_tokens: u64,
    /// Every output token, reasoning among them.
    pub output_tokens: u64,
    /// Input tokens read from the provider's cache.
    pub cache_read_input_tokens: u64,
    /// Input tokens written to the provider's cache.
    pub cache_write_input_tokens: u64,
    /// The end customer it was for, as stored; empty when none was sent.
    pub customer_id: String,
    /// The conversation it belongs to; empty when none was sent.
    pub conversation_id: String,
    /// The customer's own tags.
    pub tags: Vec<(String, String)>,
    /// A run's stall timeout, in seconds; `None` where it set none.
    pub bound_stall_after_s: Option<u32>,
    /// A run's step cap; `None` where it set none.
    pub bound_max_steps: Option<u32>,
    /// A run's cost cap, in US dollars; `None` where it set none.
    pub bound_max_cost_usd: Option<f64>,
    /// The share of runs the SDK sends, on a run; `None` on every other kind.
    pub sample_rate: Option<f64>,
    /// The attributes the contract keeps, under their own names.
    pub attributes: Vec<(String, String)>,
}

/// A closed set as its text, the shape its `LowCardinality(String)` column
/// holds: the client panics on a unit variant, so a row's enum fields are
/// written through here and read back through `FromStr`.
mod text {
    use std::fmt::Display;
    use std::str::FromStr;

    use serde::{Deserialize, Deserializer, Serializer, de};

    pub(super) fn serialize<T: Display, S: Serializer>(
        value: &T,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }

    pub(super) fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
    where
        T: FromStr,
        T::Err: Display,
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// ClickHouse as the module's own user, in its own database.
#[derive(Clone)]
pub struct Store {
    client: clickhouse::Client,
}

impl Store {
    /// The store at `url` ([`client`]). Dials nothing.
    pub fn connect(url: &str) -> anyhow::Result<Self> {
        Ok(Self {
            client: client(url)?.with_database(DATABASE),
        })
    }

    /// Write finished spans: one asynchronous insert, waited for, so `Ok`
    /// means ClickHouse holds them, the hourly rollup's rows with them.
    ///
    /// ⚠ An error's text can carry ClickHouse's own message: log its kind,
    /// never the text, beside a span's fields.
    pub async fn insert_spans(&self, spans: &[SpanRow]) -> clickhouse::error::Result<()> {
        if spans.is_empty() {
            return Ok(());
        }
        // The client sets no connect timeout of its own, and the first insert
        // holds every other's start while it reads the table's columns:
        // bounded here, as a read is.
        let insert = within(INSERT_SEND_TIMEOUT, self.client.insert::<SpanRow>("spans")).await?;
        let mut insert = insert
            .with_setting("async_insert", "1")
            .with_setting("wait_for_async_insert", "1")
            .with_timeouts(Some(INSERT_SEND_TIMEOUT), Some(INSERT_END_TIMEOUT));
        for span in spans {
            insert.write(span).await?;
        }
        insert.end().await
    }

    /// A read of `projects`' rows alone: `sql`, a literal, sent as written,
    /// which names its projects in its own `WHERE` with [`PROJECTS_PARAM`];
    /// its other values bound by the server through [`Read::param`]. The
    /// server stops it after eight seconds, and the client gives up at ten.
    pub fn read(&self, projects: &Projects, sql: &'static str) -> Read {
        let refused = if !sql.contains(PROJECTS_PARAM) {
            Some("a read names its projects in its own WHERE, with {projects:Array(String)}")
        } else if carries_settings(sql) {
            Some("a read's settings are the store's: its SQL carries no SETTINGS clause")
        } else {
            None
        };
        Read {
            query: self
                .client
                .query_raw(sql)
                .with_setting(PROJECTS_SETTING, projects.setting())
                .with_setting("max_execution_time", READ_MAX_EXECUTION_SECS)
                .param("projects", projects.ids()),
            refused,
        }
    }
}

/// Whether `sql` has a `SETTINGS` clause of its own, which would win over
/// the settings the store sets: the projects the policy reads, and the
/// server's cap on the read's time.
fn carries_settings(sql: &str) -> bool {
    sql.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|word| word.eq_ignore_ascii_case("settings"))
}

/// One read, its projects fixed when it was made: values go in through
/// [`Read::param`] alone, so nothing a caller adds can widen what it sees or
/// lift its limits. ⚠ A read built against that — SQL that does not name its
/// projects or carries settings of its own, or a second binding of them —
/// fails when fetched, in every build, before it is sent: where no row
/// policy holds the reader, as in the Compose quickstart, its own `WHERE` is
/// all there is.
#[must_use = "a read does nothing until it is fetched"]
pub struct Read {
    query: clickhouse::query::Query,
    /// Why the read may not be sent, once it was built against its rules.
    refused: Option<&'static str>,
}

impl Read {
    /// Bind `value` to `{name:Type}` in the read's SQL, on the server's side.
    /// A list for an `Array(…)` is a `Vec` or a slice: serde writes a
    /// fixed-size array as a tuple, which the server refuses as an array. A
    /// closed set binds as its text (`kind.to_string()`): serde writes a lone
    /// variant quoted, and a `String` parameter keeps the quotes, so it
    /// matches nothing.
    ///
    /// ⚠ A value from a request binds as its column's type, parsed first — a
    /// run's id as a `Uuid` into `{run:UUID}` — so one that does not parse is
    /// the caller's 400: ClickHouse's error prints a value it cannot parse
    /// unquoted, past its masking. Never an `Identifier` from a request.
    pub fn param(mut self, name: &str, value: impl Serialize) -> Self {
        if name == "projects" {
            self.refused = Some("a read's projects are fixed when it is made");
        } else {
            self.query = self.query.param(name, value);
        }
        self
    }

    /// Every row.
    pub async fn fetch_all<T: RowOwned + RowRead>(self) -> clickhouse::error::Result<Vec<T>> {
        within(READ_TIMEOUT, self.sendable()?.fetch_all()).await
    }

    /// The one row: `RowNotFound` when there is none.
    pub async fn fetch_one<T: RowOwned + RowRead>(self) -> clickhouse::error::Result<T> {
        within(READ_TIMEOUT, self.sendable()?.fetch_one()).await
    }

    /// The row, if there is one.
    pub async fn fetch_optional<T: RowOwned + RowRead>(
        self,
    ) -> clickhouse::error::Result<Option<T>> {
        within(READ_TIMEOUT, self.sendable()?.fetch_optional()).await
    }

    /// The query, unless the read was built against its rules.
    fn sendable(self) -> clickhouse::error::Result<clickhouse::query::Query> {
        match self.refused {
            None => Ok(self.query),
            Some(rule) => Err(clickhouse::error::Error::InvalidParams(rule.into())),
        }
    }
}

/// A call to ClickHouse, given up on past `limit`: the client sets no limit
/// of its own, so a server or a network that never answers would hold the
/// caller for good. The migrator's statements are bounded by it too.
pub(crate) async fn within<T>(
    limit: Duration,
    call: impl Future<Output = clickhouse::error::Result<T>>,
) -> clickhouse::error::Result<T> {
    tokio::time::timeout(limit, call)
        .await
        .unwrap_or(Err(clickhouse::error::Error::TimedOut))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The user and password are a DSN's, percent-encoded in the URL, and
    /// leave it decoded.
    #[test]
    fn a_url_gives_up_its_user_and_password() {
        let dsn = Dsn::parse("http://tele%40metry:p%3Ass%2Fw%25rd@clickhouse:8123", false).unwrap();
        assert_eq!(dsn.origin, "http://clickhouse:8123/");
        assert_eq!(dsn.user, "tele@metry");
        assert_eq!(
            dsn.password.as_ref().map(Redacted::expose),
            Some("p:ss/w%rd")
        );

        let bare = Dsn::parse("https://clickhouse.example.com/proxied", false).unwrap();
        assert_eq!(bare.origin, "https://clickhouse.example.com/proxied");
        assert_eq!(bare.user, "", "no user is ClickHouse's `default`");
        assert!(bare.password.is_none());
    }

    /// ⚠ In a pod, a ClickHouse outside the cluster is reached over `https`
    /// alone; on a laptop, anything goes.
    #[test]
    fn a_pod_reaches_clickhouse_encrypted_or_inside_the_cluster() {
        for url in [
            "https://user:pw@clickhouse.example.com",
            "http://user:pw@clickhouse.telmoni.svc:8123",
            "http://user:pw@clickhouse.telmoni.svc.cluster.local:8123",
            "http://user:pw@127.0.0.1:8123",
            "http://user:pw@[::1]:8123",
        ] {
            assert!(
                Dsn::parse(url, true).is_ok(),
                "{url:?} was refused in a pod"
            );
        }
        for url in [
            "http://user:pw@clickhouse:8123",
            "http://user:pw@clickhouse.example.com:8123",
            "http://user:pw@clickhouse.svc.example.com:8123",
            "http://user:pw@10.0.0.5:8123",
            "http://user:pw@[fd00::5]:8123",
        ] {
            let error = Dsn::parse(url, true)
                .err()
                .expect("taken in a pod")
                .to_string();
            assert!(!error.contains("pw@"), "{error}");
            assert!(
                Dsn::parse(url, false).is_ok(),
                "{url:?} was refused on a laptop"
            );
        }
    }

    #[test]
    fn a_url_that_cannot_reach_clickhouses_http_interface_is_refused() {
        for url in [
            "",
            "clickhouse:8123",
            "tcp://user:pw@clickhouse:9000",
            "postgresql://user:pw@localhost:5432/telmoni",
            "http://user:%zz@clickhouse:8123",
            "http://user:%2@clickhouse:8123",
            "http://user:pw@clickhouse:8123/?database=other",
            "http://user:pw@clickhouse:8123/?readonly=0",
            "http://user:pw@clickhouse:8123/#fragment",
        ] {
            assert!(client(url).is_err(), "{url:?} was taken");
        }
    }

    /// ⚠ The error names the fault, never the URL, which holds a password.
    #[test]
    fn a_refused_url_is_never_quoted() {
        for url in [
            "tcp://user:hunter2@clickhouse:9000",
            "http://user:hunter2%zz@clickhouse:8123",
            "http://user:hunter2@[::1",
            "http://user:hunter2@clickhouse:8123/?readonly=0",
        ] {
            let error = client(url).unwrap_err().to_string();
            assert!(!error.contains("hunter2"), "{error}");
        }
    }

    /// ⚠ A read built against its rules is refused before it is sent, in
    /// every build: nothing listens at the store's address, so one that was
    /// sent would fail another way.
    #[tokio::test]
    async fn a_read_built_against_its_rules_is_never_sent() {
        let store = Store::connect("http://127.0.0.1:9").unwrap();
        let one = Projects::one(ProjectId::from_trusted("project_a"));
        let unnamed = store
            .read(&one, "SELECT count() FROM telemetry.spans")
            .fetch_one::<u64>()
            .await;
        assert!(
            matches!(unnamed, Err(clickhouse::error::Error::InvalidParams(_))),
            "a read naming no project in its WHERE was sent: {unnamed:?}"
        );
        let widened = store
            .read(
                &one,
                "SELECT count() FROM telemetry.spans WHERE project_id IN {projects:Array(String)}",
            )
            .param("projects", vec!["project_b"])
            .fetch_one::<u64>()
            .await;
        assert!(
            matches!(widened, Err(clickhouse::error::Error::InvalidParams(_))),
            "a read's projects were bound twice and it was sent: {widened:?}"
        );
        for sql in [
            "SELECT count() FROM telemetry.spans WHERE project_id IN {projects:Array(String)} \
             SETTINGS SQL_telmoni_projects = '[\"project_b\"]'",
            "SELECT count() FROM telemetry.spans WHERE project_id IN {projects:Array(String)} \
             settings max_execution_time = 600",
        ] {
            let overridden = store.read(&one, sql).fetch_one::<u64>().await;
            assert!(
                matches!(overridden, Err(clickhouse::error::Error::InvalidParams(_))),
                "a read carrying settings of its own was sent: {overridden:?}"
            );
        }
        assert!(
            !carries_settings(
                "SELECT getSetting('max_threads') FROM telemetry.spans \
                 WHERE project_id IN {projects:Array(String)}"
            ),
            "a read of a setting is no SETTINGS clause"
        );
    }

    /// A read names at least one project, as a JSON array, so an id cannot
    /// smuggle in another.
    #[test]
    fn a_read_names_its_projects_as_a_json_array() {
        assert!(Projects::of(Vec::new()).is_none(), "a read named none");
        let one = Projects::one(ProjectId::from_trusted("project_a"));
        assert_eq!(one.setting(), r#"["project_a"]"#);
        let odd = Projects::of([
            ProjectId::from_trusted("project_a\",\"project_b"),
            ProjectId::from_trusted("project_c,project_d"),
        ])
        .unwrap();
        let parsed: Vec<String> = serde_json::from_str(&odd.setting()).unwrap();
        assert_eq!(
            parsed,
            ["project_a\",\"project_b", "project_c,project_d"],
            "an id's quote or comma became a second id"
        );
    }
}
