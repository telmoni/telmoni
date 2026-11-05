package telmoni

// The ingest contract's names, as the core fixes them in telmoni/telmoni's
// crates/telemetry/src/contract.rs: the GenAI semantic conventions' names at
// the commit it pins, ours under telmoni.*, and a call's cost under
// Langfuse's name. A name changes there first, then here.

const (
	// scope is the instrumentation scope every span the SDK opens is under.
	scope      = "telmoni"
	distroName = "telemetry.distro.name"
	distro     = "telmoni"

	attrRunID     = "telmoni.run_id"
	attrAgent     = "telmoni.agent"
	attrUserID    = "user.id"
	tagPrefix     = "telmoni.tag."
	attrCancelled = "telmoni.cancelled"
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
	// attrCost is what the call cost in US dollars, as reported: Langfuse's
	// name, since the conventions have none.
	attrCost      = "gen_ai.usage.cost"
	attrSessionID = "gen_ai.conversation.id"
	// attrTimeToFirstChunk is in seconds, from the request to the first chunk.
	attrTimeToFirstChunk = "gen_ai.response.time_to_first_chunk"

	attrRequestReasoningLevel = "gen_ai.request.reasoning.level"

	attrInputMessages      = "gen_ai.input.messages"
	attrOutputMessages     = "gen_ai.output.messages"
	attrSystemInstructions = "gen_ai.system_instructions"
	attrToolCallArguments  = "gen_ai.tool.call.arguments"
	attrToolCallResult     = "gen_ai.tool.call.result"

	runOperation          = "invoke_agent"
	toolOperation         = "execute_tool"
	defaultModelOperation = "chat"

	// contentModeHeader names the project's content mode on every answer:
	// off or on.
	contentModeHeader = "Telmoni-Content-Mode"
	// sentAtHeader names this clock as an export leaves, in nanoseconds since
	// the epoch, by which ingest moves a run from a clock a zone or a date out.
	sentAtHeader = "Telmoni-Sent-At"
)

// envCaptureContent is the variable ConfigFromEnv reads CaptureContent from:
// 1 turns it on, and anything else, or nothing, leaves it off.
const envCaptureContent = "TELMONI_CAPTURE_CONTENT"

const (
	// mediaPath is where an image or file is put, by the lowercase hex
	// SHA-256 of its bytes.
	mediaPath = "/v1/media/"
	// fileIDPrefix, then its SHA-256, is a file part's file_id naming a file
	// put to mediaPath.
	fileIDPrefix = "telmoni:"
	// mediaMaxBytes is the largest file the platform keeps: one past it is
	// named but never put.
	mediaMaxBytes = 5 * 1024 * 1024
	// mediaDefaultType is a file's Content-Type where its part names no type a
	// header can carry.
	mediaDefaultType = "application/octet-stream"
)

// mediaFields are the content fields whose messages' blob parts the exporter
// lifts out: each image or file put to mediaPath and sent as a file part
// naming it.
var mediaFields = map[string]bool{
	attrInputMessages:      true,
	attrOutputMessages:     true,
	attrSystemInstructions: true,
}

// modelOperations are the operations ingest reads as a model call; any other
// files one elsewhere.
var modelOperations = []string{"chat", "generate_content", "text_completion", "embeddings"}

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
	// environmentMax and releaseMax are the deployment's longest names:
	// ingest reads a resource naming a longer one as naming none, without a
	// word.
	environmentMax = 128
	releaseMax     = 128
	// batchMaxBytes is the largest body an export may be, contract.rs's
	// BATCH_MAX_BYTES: ingest refuses one past it whole.
	batchMaxBytes = 1 << 20
	// batchSpansMax is the most spans an export may hold, contract.rs's
	// BATCH_SPANS_MAX: ingest refuses one past it whole.
	batchSpansMax = 1000
	// attributeKeyMax and attributeValueMax are an attribute's longest key and
	// value as text, in characters, contract.rs's: ingest drops one past either.
	attributeKeyMax   = 128
	attributeValueMax = 1024
)

// content is what the exporter takes off a span unless the code asked and the
// project allows.
// attrContentCut names the content fields the exporter cut so a span fits an
// export.
const attrContentCut = "telmoni.content_cut"

// A failure, as the conventions' exception event carries it: content.
const (
	eventException       = "exception"
	attrExceptionType    = "exception.type"
	attrExceptionMessage = "exception.message"
)

// A score, as the conventions' evaluation event carries it, with ours beside.
const (
	eventEvaluation     = "gen_ai.evaluation.result"
	attrEvaluationName  = "gen_ai.evaluation.name"
	attrEvaluationValue = "gen_ai.evaluation.score.value"
	attrEvaluationLabel = "gen_ai.evaluation.score.label"
	attrScoreType       = "telmoni.score.type"
	attrScoreRangeMin   = "telmoni.score.range_min"
	attrScoreRangeMax   = "telmoni.score.range_max"
	attrScoreVersion    = "telmoni.score.version"
	// scoreSpanName and attrScoreSent are a late score's carrier, never sent:
	// the exporter takes it out of the batch and posts its item to scoresPath.
	scoreSpanName = "telmoni.score"
	attrScoreSent = "telmoni.score.sent"
	scoresPath    = "/v1/scores"
)

// scoreKeys are an evaluation event's metadata: any other key on one is
// content.
var scoreKeys = map[string]bool{
	attrEvaluationName:  true,
	attrEvaluationValue: true,
	attrEvaluationLabel: true,
	attrScoreType:       true,
	attrScoreRangeMin:   true,
	attrScoreRangeMax:   true,
	attrScoreVersion:    true,
}

const (
	// A score's longest name, category and version, in characters, scores.rs's.
	scoreNameMax     = 64
	scoreCategoryMax = 64
	scoreVersionMax  = 64
	// scoresPerSpanMax leaves the exception event room among a span's 32.
	scoresPerSpanMax = 16
	// scoresSentMax is the most scores one POST to scoresPath holds.
	scoresSentMax = 100
)

// attrStatusDescription is the key a status message is handed to the mask by.
const attrStatusDescription = "otel.status_description"

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
	attrContentCut:                true,
	attrExceptionType:             true,
	attrExceptionMessage:          true,
	"exception.stacktrace":        true,
	attrStatusDescription:         true,
	"telmoni.events":              true,
}
