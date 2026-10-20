package telmoni

// The ingest contract's names, as the core fixes them in telmoni/telmoni's
// crates/telemetry/src/contract.rs: the GenAI semantic conventions' names at
// the commit it pins, and ours under telmoni.*. A name changes there first,
// then here.

const (
	// scope is the instrumentation scope every span the SDK opens is under.
	scope      = "telmoni"
	distroName = "telemetry.distro.name"
	distro     = "telmoni"

	attrRunID           = "telmoni.run_id"
	attrAgent           = "telmoni.agent"
	attrUserID          = "user.id"
	tagPrefix           = "telmoni.tag."
	attrBoundStallAfter = "telmoni.bound.stall_after_s"
	attrBoundMaxSteps   = "telmoni.bound.max_steps"
	attrBoundMaxCostUSD = "telmoni.bound.max_cost_usd"
	attrSampleRate      = "telmoni.sample_rate"
	attrCancelled       = "telmoni.cancelled"
	// attrSampledOut is never sent: on every span of a run the sample rate
	// kept out, from its start, so the processor drops each however late it
	// ends.
	attrSampledOut = "telmoni.sampled_out"
	// runStart is the span of no duration that opens a run, sent at once.
	runStart = "telmoni.run.start"
	// The deployment, on the resource: the configuration's Environment and
	// Release.
	attrEnvironment = "deployment.environment.name"
	attrRelease     = "service.version"
	// The W3C headers a run crosses services in: the span's context, and the
	// run's id and agent as baggage members.
	headerTraceparent = "traceparent"
	headerBaggage     = "baggage"

	attrOperation             = "gen_ai.operation.name"
	attrAgentName             = "gen_ai.agent.name"
	attrToolName              = "gen_ai.tool.name"
	attrErrorType             = "error.type"
	attrResponseModel         = "gen_ai.response.model"
	attrRequestModel          = "gen_ai.request.model"
	attrProvider              = "gen_ai.provider.name"
	attrInputTokens           = "gen_ai.usage.input_tokens"
	attrOutputTokens          = "gen_ai.usage.output_tokens"
	attrCacheReadInputTokens  = "gen_ai.usage.cache_read.input_tokens"
	attrCacheWriteInputTokens = "gen_ai.usage.cache_write.input_tokens"
	attrSessionID             = "gen_ai.conversation.id"

	attrInputMessages      = "gen_ai.input.messages"
	attrOutputMessages     = "gen_ai.output.messages"
	attrSystemInstructions = "gen_ai.system_instructions"
	attrToolCallArguments  = "gen_ai.tool.call.arguments"
	attrToolCallResult     = "gen_ai.tool.call.result"

	runOperation          = "invoke_agent"
	toolOperation         = "execute_tool"
	defaultModelOperation = "chat"

	// contentModeHeader names the project's content mode on every answer:
	// off, on or sealed.
	contentModeHeader = "Telmoni-Content-Mode"
	// sentAtHeader names this clock as an export leaves, in nanoseconds since
	// the epoch, by which ingest moves a run from a clock a zone or a date out.
	sentAtHeader = "Telmoni-Sent-At"
)

const (
	// tagsMax is the most tags ingest keeps a span, contract.rs's TAGS_MAX.
	tagsMax = 32
	// tagKeyMax is a tag key's most characters, contract.rs's TAG_KEY_MAX.
	tagKeyMax = 64
	// tagValueMax is a tag value's most characters, contract.rs's TAG_VALUE_MAX.
	tagValueMax = 256
	// nameMax is a span's name's most characters, a step's or a tool's,
	// contract.rs's NAME_MAX: room for a model call's "<operation> <model>".
	nameMax = 145
	// The rest of ingest's caps, in characters, each named after its field:
	// it refuses a span past any.
	agentMax     = 128
	errorTypeMax = 128
	modelMax     = 128
	providerMax  = 128
	userIDMax    = 128
	sessionIDMax = 200
	// batchMaxBytes is the largest body an export may be, contract.rs's
	// BATCH_MAX_BYTES: ingest refuses one past it whole.
	batchMaxBytes = 1 << 20
	// attributeKeyMax and attributeValueMax are an attribute's longest key and
	// value as text, in characters, contract.rs's: ingest drops one past either.
	attributeKeyMax   = 128
	attributeValueMax = 1024
)

// content is what the exporter takes off a span unless the code asked and the
// project allows.
var content = map[string]bool{
	attrInputMessages:             true,
	attrOutputMessages:            true,
	attrSystemInstructions:        true,
	"gen_ai.tool.definitions":     true,
	attrToolCallArguments:         true,
	attrToolCallResult:            true,
	"gen_ai.retrieval.query.text": true,
	"gen_ai.retrieval.documents":  true,
	"gen_ai.memory.query.text":    true,
	"gen_ai.memory.records":       true,
	"gen_ai.prompt.variable":      true,
	"user.email":                  true,
	"user.full_name":              true,
	"user.name":                   true,
}
