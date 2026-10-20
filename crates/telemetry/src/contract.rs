//! The ingest contract: what `POST /otlp/v1/traces` reads from a span and
//! what it keeps, written once, so the SDKs, the field inventory and the
//! docs are held to one list. The names are the GenAI conventions' at the
//! pinned commit ([`CONVENTIONS_COMMIT`]); what they do not name is ours,
//! under `telmoni.*`.
//!
//! Every cap is a constant named after its field. A span that breaks a
//! field's rule is refused whole; an attribute past its cap is dropped and
//! the span kept, since a span lost for one attribute loses its run.

use std::fmt;

use serde::Serialize;

use telmoni_shared::SpanKind;

/// The GenAI semantic conventions the names below are taken from:
/// `open-telemetry/semantic-conventions-genai`, which has tagged no release.
pub const CONVENTIONS_COMMIT: &str = "6fd0d763a092db245a54104fc1124031eb3c51d2";

// ── The batch ────────────────────────────────────────────────────────────────

/// The most spans one export may hold.
pub const BATCH_SPANS_MAX: usize = 1_000;
/// The largest body, once decompressed.
pub const BATCH_MAX_BYTES: usize = 1024 * 1024;
/// How much of the answer's `error_message` names what was refused or
/// dropped before a count stands in for the rest.
pub const ERROR_MESSAGE_MAX: usize = 4 * 1024;
/// How many exports a project may send a minute, and at once: one every
/// 100 ms sustained, a burst of a minute's worth.
pub const EXPORTS_PER_MINUTE: u32 = 600;
/// How long a body's digest is held while its rows are being written: a
/// copy that arrives inside it is told to retry rather than written twice.
pub const CLAIM_PENDING_SECS: u64 = 60;
/// How long a written body's retry is answered, not written: an exporter's
/// retry follows a lost answer within minutes.
pub const CLAIM_ANSWER_SECS: u64 = 60 * 60;
/// How long after its export arrived a trace's last span may end: later, its
/// clock is a zone or a date out rather than drifting, and the trace moves back.
pub const CLOCK_AHEAD_MAX_SECS: i64 = 55 * 60;

// ── The ping lane ────────────────────────────────────────────────────────────

/// The most of a ping's body that is read. A body is content — what 4.1
/// keeps on an `on` project, and what is dropped until then, the answer
/// saying so — and is never read past this, so a crontab's `curl` with a log
/// attached costs a bounded read; the ping counts whatever its body.
pub const PING_BODY_MAX_BYTES: usize = 100 * 1024;
/// A run's progress as its `/checkpoint`s reported it, on its finished span:
/// its steps, and its spend in US dollars, under names of ours, since no
/// convention names what a crontab reports.
pub const RUN_STEPS: &str = "telmoni.run.steps";
pub const RUN_COST_USD: &str = "telmoni.run.cost_usd";
/// A `/fail` ping's exit code, under the conventions' own name for a
/// process's.
pub const EXIT_CODE: &str = "process.exit.code";

// ── Caps, each named after its field ─────────────────────────────────────────

/// A span's `name`, a run's and a tool's among them: room for a model call's
/// `<operation> <model>`, the longest operation and a model at its cap.
pub const NAME_MAX: usize = longest(&MODEL_OPERATIONS) + 1 + MODEL_MAX;
/// `agent`, the monitor a run counts under: the monitors' and open runs' CHECK.
pub const AGENT_MAX: usize = 128;
/// `error_type`: the exception's class.
pub const ERROR_TYPE_MAX: usize = 128;
/// `model`, as the provider spells it.
pub const MODEL_MAX: usize = 128;
/// `provider`, as the conventions spell it.
pub const PROVIDER_MAX: usize = 128;
/// `user_id`, as sent, before it is hashed.
pub const USER_ID_MAX: usize = 128;
/// `session_id`, kept as sent.
pub const SESSION_ID_MAX: usize = 200;
/// `environment`, as sent; longer, the resource is read as naming none.
pub const ENVIRONMENT_MAX: usize = 128;
/// `release`, as sent; longer, the resource is read as naming none.
pub const RELEASE_MAX: usize = 128;
/// How many `tags` a span may carry.
pub const TAGS_MAX: usize = 32;
/// A tag's key, after `telmoni.tag.`.
pub const TAG_KEY_MAX: usize = 64;
/// A tag's value.
pub const TAG_VALUE_MAX: usize = 256;
/// How many `attributes` a span may keep, its resource's among them.
pub const ATTRIBUTES_MAX: usize = 64;
/// An attribute's key.
pub const ATTRIBUTE_KEY_MAX: usize = 128;
/// An attribute's value, a named array's JSON among them.
pub const ATTRIBUTE_VALUE_MAX: usize = 1_024;
/// What a resource's kept attributes may take together, keys and values in
/// bytes: every span of its export carries a copy, so a resource near the
/// per-attribute caps would cost a thousand times its size.
pub const RESOURCE_ATTRIBUTES_BYTES_MAX: usize = 2 * 1_024;

// ── Units ────────────────────────────────────────────────────────────────────

/// What one span counts against its plan: one unit per started KiB of its
/// fields as ingest keeps them, content apart.
pub const SPAN_UNIT_BYTES: u32 = 1_024;
/// What a span's ids, times and counts take, whatever its text.
pub const SPAN_FIXED_BYTES: u32 = 64;

// ── Ours, on the wire ────────────────────────────────────────────────────────

/// The run a span belongs to: a UUID, the same on every span of the run.
pub const RUN_ID: &str = "telmoni.run_id";
/// The run's agent: the monitor it is counted under.
pub const AGENT: &str = "telmoni.agent";
/// The end user a span was for, as the application's own id for them:
/// OpenTelemetry's own name, so an application instrumented without our SDK
/// is read the same. Never who signs in to the console, and never the
/// organization that pays: an id, hashed before it is a row unless the
/// project keeps ids plain, and the one `user.*` attribute that is not
/// content.
pub const USER_ID: &str = "user.id";
/// A tag: `telmoni.tag.<key>`.
pub const TAG_PREFIX: &str = "telmoni.tag.";
/// A run's stall timeout, in seconds.
pub const BOUND_STALL_AFTER_S: &str = "telmoni.bound.stall_after_s";
/// A run's step cap.
pub const BOUND_MAX_STEPS: &str = "telmoni.bound.max_steps";
/// A run's cost cap, in US dollars.
pub const BOUND_MAX_COST_USD: &str = "telmoni.bound.max_cost_usd";
/// The share of runs the SDK sends.
pub const SAMPLE_RATE: &str = "telmoni.sample_rate";
/// Set on a span the agent cancelled.
pub const CANCELLED: &str = "telmoni.cancelled";
/// `sealed` on a span whose content our SDK encrypted (4.2).
pub const CONTENT_MARK: &str = "telmoni.content";
/// The span of no duration that opens a run, sent at once; never a row.
pub const RUN_START: &str = "telmoni.run.start";
/// Our SDKs' instrumentation scope.
pub const SCOPE: &str = "telmoni";
/// The resource attribute our SDKs name themselves by.
pub const DISTRO_NAME: &str = "telemetry.distro.name";
/// Its value.
pub const DISTRO: &str = "telmoni";

// ── The conventions' names a column is read from ─────────────────────────────

/// What the span does: decides its kind ([`kind_of_operation`]).
pub const OPERATION: &str = "gen_ai.operation.name";
/// A run's name, on a run of one agent.
pub const AGENT_NAME: &str = "gen_ai.agent.name";
/// A run's name, on a run of several agents.
pub const WORKFLOW_NAME: &str = "gen_ai.workflow.name";
/// A tool call's name.
pub const TOOL_NAME: &str = "gen_ai.tool.name";
/// The exception's class on an error.
pub const ERROR_TYPE: &str = "error.type";
/// The model that answered.
pub const RESPONSE_MODEL: &str = "gen_ai.response.model";
/// The model asked for, where the response names none.
pub const REQUEST_MODEL: &str = "gen_ai.request.model";
/// The provider.
pub const PROVIDER: &str = "gen_ai.provider.name";
/// Every input token.
pub const INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
/// Every output token.
pub const OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";
/// Input tokens read from the provider's cache.
pub const CACHE_READ_INPUT_TOKENS: &str = "gen_ai.usage.cache_read.input_tokens";
/// Input tokens written to the provider's cache.
pub const CACHE_WRITE_INPUT_TOKENS: &str = "gen_ai.usage.cache_write.input_tokens";
/// The session a span belongs to, under the conventions' name for it.
pub const SESSION_ID: &str = "gen_ai.conversation.id";

/// The names the pinned conventions replaced, read as what replaced them,
/// since exporters lag the conventions: a rename is an edit here, never to
/// `spans`.
pub const REPLACED: &[(&str, &str)] = &[
    ("gen_ai.system", PROVIDER),
    ("gen_ai.usage.prompt_tokens", INPUT_TOKENS),
    ("gen_ai.usage.completion_tokens", OUTPUT_TOKENS),
    (
        "gen_ai.usage.cache_creation.input_tokens",
        CACHE_WRITE_INPUT_TOKENS,
    ),
];

/// The name the pinned conventions give `key`: itself, unless they replaced it.
#[must_use]
pub fn current_name(key: &str) -> &str {
    REPLACED
        .iter()
        .find(|(old, _)| *old == key)
        .map_or(key, |(_, now)| now)
}

/// The operations a span is read as a model call by.
pub const MODEL_OPERATIONS: [&str; 4] =
    ["chat", "generate_content", "text_completion", "embeddings"];

/// The kind a span's operation makes it, or `None` for a span the
/// conventions give no operation: a step under our scope, a span elsewhere.
#[must_use]
pub fn kind_of_operation(operation: &str) -> Option<SpanKind> {
    match operation {
        "invoke_agent" | "invoke_workflow" => Some(SpanKind::Run),
        "execute_tool" => Some(SpanKind::ToolCall),
        _ if MODEL_OPERATIONS.contains(&operation) => Some(SpanKind::ModelCall),
        _ => None,
    }
}

/// The longest of `names` in bytes, which for the conventions' ASCII names
/// is characters.
const fn longest(names: &[&str]) -> usize {
    let mut most = 0;
    let mut rest = names;
    while let [first, tail @ ..] = rest {
        if first.len() > most {
            most = first.len();
        }
        rest = tail;
    }
    most
}

// ── Content ──────────────────────────────────────────────────────────────────

/// What a user wrote or a model produced, and the people it names: never a
/// column; dropped at the door on an `off` project, the span kept.
pub const CONTENT: &[&str] = &[
    "gen_ai.input.messages",
    "gen_ai.output.messages",
    "gen_ai.system_instructions",
    "gen_ai.tool.definitions",
    "gen_ai.tool.call.arguments",
    "gen_ai.tool.call.result",
    "gen_ai.retrieval.query.text",
    "gen_ai.retrieval.documents",
    "gen_ai.memory.query.text",
    "gen_ai.memory.records",
    "gen_ai.prompt.variable",
    "user.email",
    "user.full_name",
    "user.name",
];

/// Whether `key` is content.
#[must_use]
pub fn is_content(key: &str) -> bool {
    CONTENT.contains(&key)
}

// ── The field inventory ──────────────────────────────────────────────────────

/// The span attributes kept in `attributes` under their own names, whatever
/// their value's type; a named array is kept as its JSON.
pub const INVENTORY_SPAN: &[&str] = &[
    "gen_ai.response.id",
    "gen_ai.response.finish_reasons",
    "gen_ai.response.status",
    "gen_ai.request.reasoning.level",
    "gen_ai.agent.version",
    "gen_ai.tool.call.id",
    "gen_ai.tool.type",
    "gen_ai.output.type",
    "gen_ai.prompt.name",
    "gen_ai.prompt.version",
    "gen_ai.data_source.id",
    "server.address",
];

/// The deployment a span was sent from, on its resource: our SDKs set it
/// from their `environment` option or `TELMONI_ENVIRONMENT`.
pub const ENVIRONMENT: &str = "deployment.environment.name";
/// The deployment's release, on the resource: the SDKs' `release` option or
/// `TELMONI_RELEASE`.
pub const RELEASE: &str = "service.version";

/// The resource attributes a column is read from: consumed, so neither is an
/// attribute kept.
pub const RESOURCE_FIELDS: &[&str] = &[ENVIRONMENT, RELEASE];

/// The resource attributes kept in `attributes` under their own names.
pub const INVENTORY_RESOURCE: &[&str] = &["service.name"];

/// The resource attributes kept by their prefix: what the SDK and the
/// distribution say about themselves.
pub const INVENTORY_RESOURCE_PREFIXES: &[&str] = &["telemetry.sdk.", "telemetry.distro."];

/// Whether the inventory names a span attribute.
#[must_use]
pub fn is_inventory_span(key: &str) -> bool {
    INVENTORY_SPAN.contains(&key)
}

/// Whether the inventory names a resource attribute.
#[must_use]
pub fn is_inventory_resource(key: &str) -> bool {
    INVENTORY_RESOURCE.contains(&key)
        || INVENTORY_RESOURCE_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
}

/// Every attribute a column is read from: consumed, so none of them is
/// also an attribute kept or dropped.
pub const FIELDS: &[&str] = &[
    RUN_ID,
    AGENT,
    USER_ID,
    BOUND_STALL_AFTER_S,
    BOUND_MAX_STEPS,
    BOUND_MAX_COST_USD,
    SAMPLE_RATE,
    CANCELLED,
    CONTENT_MARK,
    OPERATION,
    AGENT_NAME,
    WORKFLOW_NAME,
    TOOL_NAME,
    ERROR_TYPE,
    RESPONSE_MODEL,
    REQUEST_MODEL,
    PROVIDER,
    INPUT_TOKENS,
    OUTPUT_TOKENS,
    CACHE_READ_INPUT_TOKENS,
    CACHE_WRITE_INPUT_TOKENS,
    SESSION_ID,
];

/// Whether a column is read from `key`.
#[must_use]
pub fn is_field(key: &str) -> bool {
    FIELDS.contains(&key) || key.starts_with(TAG_PREFIX)
}

// ── The inventory, as a page reads it ────────────────────────────────────────
//
// One row for every name above, so the docs page (`api/field-inventory.mdx`,
// held to [`INVENTORY`] by `tests/field_inventory.rs`) and the console's
// privacy page, which reads it over `/internal/v1/telemetry/field-inventory`,
// say what the code does and nothing else. The lists above are what ingest
// reads; this table is what a person reads of them, and a test holds the two
// to each other.

/// The spans an attribute belongs on, where our SDKs and the conventions put
/// it. Ingest reads it from whichever span carries it, a model's name or
/// tokens on a step among them, and keeps it as its row says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOn {
    /// Every span of a run, the run among them.
    EverySpan,
    /// The run's own span, and the `telmoni.run.start` that opens it.
    Run,
    /// A model call.
    ModelCall,
    /// A tool call.
    ToolCall,
    /// Any span that carries it.
    AnySpan,
    /// The resource every span of an export shares.
    Resource,
}

impl ReadOn {
    /// Every place, in the page's order.
    #[must_use]
    pub const fn all() -> [Self; 6] {
        [
            Self::EverySpan,
            Self::Run,
            Self::ModelCall,
            Self::ToolCall,
            Self::AnySpan,
            Self::Resource,
        ]
    }

    /// The words the page gives it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EverySpan => "every span",
            Self::Run => "the run",
            Self::ModelCall => "a model call",
            Self::ToolCall => "a tool call",
            Self::AnySpan => "any span",
            Self::Resource => "the resource",
        }
    }
}

impl fmt::Display for ReadOn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ReadOn {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The content mode a project needs for an attribute to be kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Needs {
    /// Any: metadata is kept in every mode.
    AnyMode,
    /// `on`, or `sealed` for what our SDK sealed: content.
    Content,
}

impl Needs {
    /// Every answer, in the page's order.
    #[must_use]
    pub const fn all() -> [Self; 2] {
        [Self::AnyMode, Self::Content]
    }

    /// The words the page gives it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AnyMode => "any",
            Self::Content => "on, or sealed from Telmoni's SDK",
        }
    }
}

impl fmt::Display for Needs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Needs {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// One row of the field inventory: an attribute ingest reads, the spans it
/// is read on, what becomes of it, the content mode it needs, and its rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct InventoryRow {
    /// The attribute as the wire spells it; a family by its prefix
    /// (`telmoni.tag.<key>`, `telemetry.sdk.*`); the three rows that name no
    /// attribute say what they cover in words.
    pub attribute: &'static str,
    /// The spans it is read on.
    pub on: ReadOn,
    /// What becomes of it: the column it fills, `attributes`, or the drop.
    pub kept: &'static str,
    /// The content mode a project needs for it to be kept.
    pub needs: Needs,
    /// Its rule: a cap, a format, or what refuses the span.
    pub rule: &'static str,
}

/// How the page spells a tag: any key under [`TAG_PREFIX`].
pub const TAG_FAMILY: &str = "telmoni.tag.<key>";

const KEPT_ATTRIBUTE: &str = "attributes, under its own name";
const RULE_ATTRIBUTE: &str = "a key up to 128 characters and a value up to 1,024, an array as its JSON; \
                              up to 64 attributes a span, its resource's among them, which keep \
                              2 KiB together";
const KEPT_CONTENT: &str = "dropped at the door on a project whose content mode is off, the span kept and \
                            the field named in the answer; kept in Postgres alone, never in a column, once \
                            a project may keep content";
const RULE_CONTENT: &str = "content: what a person or a model wrote";
const RULE_PERSON: &str = "names a person: content, whatever else the span carries";
const RULE_NAME: &str = "1 to 145 characters";
const RULE_UP_TO_128: &str = "up to 128 characters";
const RULE_COUNT: &str = "a whole number, 0 or more";
const RULE_REPLACED: &str = "a name the conventions replaced, read as the current one";

const fn row(
    attribute: &'static str,
    on: ReadOn,
    kept: &'static str,
    needs: Needs,
    rule: &'static str,
) -> InventoryRow {
    InventoryRow {
        attribute,
        on,
        kept,
        needs,
        rule,
    }
}

/// Every attribute ingest reads, in the page's order: ours, the conventions'
/// fields, the names they replaced, what is kept under its own name, content,
/// and the three rules for everything else.
pub const INVENTORY: &[InventoryRow] = &[
    // Ours.
    row(
        RUN_ID,
        ReadOn::EverySpan,
        "the column run_id",
        Needs::AnyMode,
        "a UUID, the same on every span of a run; a span without it is refused",
    ),
    row(
        AGENT,
        ReadOn::EverySpan,
        "the column agent, and a run's name where gen_ai.agent.name and gen_ai.workflow.name are \
         unset",
        Needs::AnyMode,
        "1 to 128 characters; a span without it is refused",
    ),
    row(
        USER_ID,
        ReadOn::AnySpan,
        "the column user_id, as its HMAC-SHA-256 under a key of the project's own, the first \
         16 bytes in hex, unless the project keeps it plain",
        Needs::AnyMode,
        RULE_UP_TO_128,
    ),
    row(
        TAG_FAMILY,
        ReadOn::AnySpan,
        "tags, under <key>",
        Needs::AnyMode,
        "up to 32 tags; a key up to 64 characters, a value up to 256",
    ),
    row(
        BOUND_STALL_AFTER_S,
        ReadOn::Run,
        "the column bound_stall_after_s",
        Needs::AnyMode,
        "seconds, a whole number",
    ),
    row(
        BOUND_MAX_STEPS,
        ReadOn::Run,
        "the column bound_max_steps",
        Needs::AnyMode,
        "a whole number",
    ),
    row(
        BOUND_MAX_COST_USD,
        ReadOn::Run,
        "the column bound_max_cost_usd",
        Needs::AnyMode,
        "US dollars",
    ),
    row(
        SAMPLE_RATE,
        ReadOn::Run,
        "the column sample_rate",
        Needs::AnyMode,
        "above 0, up to 1: the share of runs the SDK sends",
    ),
    row(
        CANCELLED,
        ReadOn::AnySpan,
        "the column status, as cancelled",
        Needs::AnyMode,
        "true on a span the agent cancelled",
    ),
    row(
        CONTENT_MARK,
        ReadOn::AnySpan,
        "read and not kept: sealed marks content our SDK encrypted, which a sealed project alone \
         keeps",
        Needs::AnyMode,
        "sealed",
    ),
    // The conventions' fields.
    row(
        OPERATION,
        ReadOn::EverySpan,
        "the column kind",
        Needs::AnyMode,
        "invoke_agent or invoke_workflow is the run; chat, generate_content, text_completion or \
         embeddings a model call; execute_tool a tool call; any other span a step under the \
         telmoni scope, a plain span elsewhere",
    ),
    row(
        AGENT_NAME,
        ReadOn::Run,
        "the column name",
        Needs::AnyMode,
        RULE_NAME,
    ),
    row(
        WORKFLOW_NAME,
        ReadOn::Run,
        "the column name, where gen_ai.agent.name is unset",
        Needs::AnyMode,
        RULE_NAME,
    ),
    row(
        TOOL_NAME,
        ReadOn::ToolCall,
        "the column name",
        Needs::AnyMode,
        RULE_NAME,
    ),
    row(
        ERROR_TYPE,
        ReadOn::AnySpan,
        "the column error_type",
        Needs::AnyMode,
        "up to 128 characters: the exception's class, never its message",
    ),
    row(
        RESPONSE_MODEL,
        ReadOn::ModelCall,
        "the column model",
        Needs::AnyMode,
        "up to 128 characters, as the provider spells it",
    ),
    row(
        REQUEST_MODEL,
        ReadOn::ModelCall,
        "the column model, where the response names none",
        Needs::AnyMode,
        "up to 128 characters, as the provider spells it",
    ),
    row(
        PROVIDER,
        ReadOn::ModelCall,
        "the column provider",
        Needs::AnyMode,
        "up to 128 characters, as the conventions spell it: openai, anthropic, gcp.vertex_ai, \
         aws.bedrock",
    ),
    row(
        INPUT_TOKENS,
        ReadOn::ModelCall,
        "the column input_tokens",
        Needs::AnyMode,
        "a whole number, 0 or more: every input token, the cache's reads and writes among them",
    ),
    row(
        OUTPUT_TOKENS,
        ReadOn::ModelCall,
        "the column output_tokens",
        Needs::AnyMode,
        "a whole number, 0 or more: every output token, reasoning among them",
    ),
    row(
        CACHE_READ_INPUT_TOKENS,
        ReadOn::ModelCall,
        "the column cache_read_input_tokens",
        Needs::AnyMode,
        RULE_COUNT,
    ),
    row(
        CACHE_WRITE_INPUT_TOKENS,
        ReadOn::ModelCall,
        "the column cache_write_input_tokens",
        Needs::AnyMode,
        RULE_COUNT,
    ),
    row(
        SESSION_ID,
        ReadOn::AnySpan,
        "the column session_id",
        Needs::AnyMode,
        "up to 200 characters, kept as sent",
    ),
    row(
        ENVIRONMENT,
        ReadOn::Resource,
        "the column environment, on every span of the export",
        Needs::AnyMode,
        "up to 128 characters, as sent: production, staging; longer, read as none",
    ),
    row(
        RELEASE,
        ReadOn::Resource,
        "the column release, on every span of the export",
        Needs::AnyMode,
        "up to 128 characters, as sent: a version or a commit; longer, read as none",
    ),
    // The names the conventions replaced.
    row(
        "gen_ai.system",
        ReadOn::ModelCall,
        "read as gen_ai.provider.name",
        Needs::AnyMode,
        RULE_REPLACED,
    ),
    row(
        "gen_ai.usage.prompt_tokens",
        ReadOn::ModelCall,
        "read as gen_ai.usage.input_tokens",
        Needs::AnyMode,
        RULE_REPLACED,
    ),
    row(
        "gen_ai.usage.completion_tokens",
        ReadOn::ModelCall,
        "read as gen_ai.usage.output_tokens",
        Needs::AnyMode,
        RULE_REPLACED,
    ),
    row(
        "gen_ai.usage.cache_creation.input_tokens",
        ReadOn::ModelCall,
        "read as gen_ai.usage.cache_write.input_tokens",
        Needs::AnyMode,
        RULE_REPLACED,
    ),
    // Kept under their own names.
    row(
        "gen_ai.response.id",
        ReadOn::ModelCall,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "gen_ai.response.finish_reasons",
        ReadOn::ModelCall,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "gen_ai.response.status",
        ReadOn::ModelCall,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "gen_ai.request.reasoning.level",
        ReadOn::ModelCall,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "gen_ai.agent.version",
        ReadOn::Run,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "gen_ai.tool.call.id",
        ReadOn::ToolCall,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "gen_ai.tool.type",
        ReadOn::ToolCall,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "gen_ai.output.type",
        ReadOn::ModelCall,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "gen_ai.prompt.name",
        ReadOn::AnySpan,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "gen_ai.prompt.version",
        ReadOn::AnySpan,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "gen_ai.data_source.id",
        ReadOn::AnySpan,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "server.address",
        ReadOn::AnySpan,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "service.name",
        ReadOn::Resource,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "telemetry.sdk.*",
        ReadOn::Resource,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    row(
        "telemetry.distro.*",
        ReadOn::Resource,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        RULE_ATTRIBUTE,
    ),
    // Content.
    row(
        "gen_ai.input.messages",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_CONTENT,
    ),
    row(
        "gen_ai.output.messages",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_CONTENT,
    ),
    row(
        "gen_ai.system_instructions",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_CONTENT,
    ),
    row(
        "gen_ai.tool.definitions",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_CONTENT,
    ),
    row(
        "gen_ai.tool.call.arguments",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_CONTENT,
    ),
    row(
        "gen_ai.tool.call.result",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_CONTENT,
    ),
    row(
        "gen_ai.retrieval.query.text",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_CONTENT,
    ),
    row(
        "gen_ai.retrieval.documents",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_CONTENT,
    ),
    row(
        "gen_ai.memory.query.text",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_CONTENT,
    ),
    row(
        "gen_ai.memory.records",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_CONTENT,
    ),
    row(
        "gen_ai.prompt.variable",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_CONTENT,
    ),
    row(
        "user.email",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_PERSON,
    ),
    row(
        "user.full_name",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_PERSON,
    ),
    row(
        "user.name",
        ReadOn::AnySpan,
        KEPT_CONTENT,
        Needs::Content,
        RULE_PERSON,
    ),
    // Everything else.
    row(
        "any other number or boolean",
        ReadOn::AnySpan,
        KEPT_ATTRIBUTE,
        Needs::AnyMode,
        "a measurement no person writes; on the resource, dropped without a word",
    ),
    row(
        "any other text or array",
        ReadOn::AnySpan,
        "dropped in every mode, the span kept and the attribute named in the answer as unlisted",
        Needs::AnyMode,
        "content arrives under names no convention gave it; on the resource, dropped without a word",
    ),
    row(
        "the span's events and status message",
        ReadOn::AnySpan,
        "never kept",
        Needs::AnyMode,
        "where an exception writes its message and stack trace",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A name in two lists would be read twice, once as a column and once
    /// as something to keep or drop.
    #[test]
    fn the_lists_name_nothing_twice() {
        let lists: [(&str, &[&str]); 5] = [
            ("fields", FIELDS),
            ("fields (resource)", RESOURCE_FIELDS),
            ("content", CONTENT),
            ("inventory (span)", INVENTORY_SPAN),
            ("inventory (resource)", INVENTORY_RESOURCE),
        ];
        let mut seen: Vec<(&str, &str)> = Vec::new();
        for (list, names) in lists {
            for name in names {
                if let Some((other, _)) = seen.iter().find(|(_, n)| n == name) {
                    panic!("{name} is in {list} and in {other}");
                }
                seen.push((list, name));
            }
        }
        for (old, now) in REPLACED {
            assert!(
                FIELDS.contains(now),
                "{old} is replaced by {now}, which no column reads"
            );
            assert!(
                !FIELDS.contains(old),
                "{old} is both a field and a replaced name"
            );
        }
        for name in INVENTORY_RESOURCE.iter().chain(RESOURCE_FIELDS) {
            assert!(
                !INVENTORY_RESOURCE_PREFIXES
                    .iter()
                    .any(|p| name.starts_with(p)),
                "{name} is named and matched by prefix"
            );
        }
    }

    #[test]
    fn a_replaced_name_reads_as_its_replacement() {
        assert_eq!(current_name("gen_ai.system"), PROVIDER);
        assert_eq!(current_name("gen_ai.usage.prompt_tokens"), INPUT_TOKENS);
        assert_eq!(current_name(PROVIDER), PROVIDER);
        assert_eq!(current_name("gen_ai.response.id"), "gen_ai.response.id");
    }

    #[test]
    fn every_operation_the_conventions_name_has_a_kind() {
        for (operation, kind) in [
            ("invoke_agent", SpanKind::Run),
            ("invoke_workflow", SpanKind::Run),
            ("chat", SpanKind::ModelCall),
            ("generate_content", SpanKind::ModelCall),
            ("text_completion", SpanKind::ModelCall),
            ("embeddings", SpanKind::ModelCall),
            ("execute_tool", SpanKind::ToolCall),
        ] {
            assert_eq!(kind_of_operation(operation), Some(kind), "{operation}");
        }
        assert_eq!(kind_of_operation("create_agent"), None);
        assert_eq!(kind_of_operation(""), None);
    }

    /// A model it takes never costs a call its name, `<operation> <model>`.
    #[test]
    fn a_model_calls_name_holds_a_model_at_its_cap() {
        for operation in MODEL_OPERATIONS {
            assert_eq!(kind_of_operation(operation), Some(SpanKind::ModelCall));
            assert!(
                operation.chars().count() + 1 + MODEL_MAX <= NAME_MAX,
                "{operation}"
            );
        }
        assert_eq!(NAME_MAX, 145, "each SDK's contract copies the number");
    }

    /// ⚠ The people a span names are content, whatever a project's mode.
    #[test]
    fn a_person_is_content() {
        for key in ["user.email", "user.full_name", "user.name"] {
            assert!(is_content(key), "{key}");
        }
        assert!(!is_content("user.id"), "an id names nobody by itself");
    }

    /// The deployment's two names are columns, so neither is an attribute kept.
    #[test]
    fn the_resource_inventory_matches_by_name_and_by_prefix() {
        assert!(is_inventory_resource("service.name"));
        assert!(is_inventory_resource("telemetry.sdk.language"));
        assert!(is_inventory_resource("telemetry.distro.version"));
        assert!(!is_inventory_resource("process.owner"));
        assert!(!is_inventory_resource("host.name"));
        assert!(!is_inventory_resource(ENVIRONMENT));
        assert!(!is_inventory_resource(RELEASE));
    }

    /// ⚠ The page's table and ingest's lists are held to each other: a name
    /// ingest reads with no row is undocumented, and a row ingest never reads
    /// is a promise; the three rows that name no attribute are the rules for
    /// everything else, and there are three.
    #[test]
    fn the_inventory_names_every_attribute_ingest_reads_and_nothing_else() {
        let mut expected: Vec<String> = FIELDS
            .iter()
            .chain(RESOURCE_FIELDS)
            .chain(CONTENT)
            .chain(INVENTORY_SPAN)
            .chain(INVENTORY_RESOURCE)
            .map(|name| (*name).to_owned())
            .chain(REPLACED.iter().map(|(old, _)| (*old).to_owned()))
            .chain(INVENTORY_RESOURCE_PREFIXES.iter().map(|p| format!("{p}*")))
            .chain(std::iter::once(TAG_FAMILY.to_owned()))
            .collect();
        expected.sort_unstable();
        assert!(TAG_FAMILY.starts_with(TAG_PREFIX));

        let (named, rules): (Vec<&InventoryRow>, Vec<&InventoryRow>) =
            INVENTORY.iter().partition(|r| r.attribute.contains('.'));
        let mut listed: Vec<String> = named.iter().map(|r| r.attribute.to_owned()).collect();
        listed.sort_unstable();
        let mut once = listed.clone();
        once.dedup();
        assert_eq!(listed, once, "a name has two rows");
        assert_eq!(listed, expected, "the inventory and ingest's lists differ");
        assert_eq!(rules.len(), 3, "the rules for everything else: {rules:?}");
        for row in INVENTORY {
            assert!(
                !row.kept.is_empty() && !row.rule.is_empty(),
                "{}",
                row.attribute
            );
        }
        for on in ReadOn::all() {
            assert!(
                INVENTORY.iter().any(|r| r.on == on),
                "no row is read on {on}"
            );
        }
        for needs in Needs::all() {
            assert!(
                INVENTORY.iter().any(|r| r.needs == needs),
                "no row needs {needs}"
            );
        }
    }

    #[test]
    fn a_row_serializes_its_closed_sets_as_their_words() {
        let json = serde_json::to_value(INVENTORY[0]).expect("a row serializes");
        assert_eq!(json["attribute"], RUN_ID);
        assert_eq!(json["on"], "every span");
        assert_eq!(json["needs"], "any");
    }
}
