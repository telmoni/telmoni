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
	// runStart is the span of no duration that opens a run, sent at once.
	runStart = "telmoni.run.start"

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

// runOperations are the operations that make a span a run's own.
var runOperations = map[string]bool{"invoke_agent": true, "invoke_workflow": true}
