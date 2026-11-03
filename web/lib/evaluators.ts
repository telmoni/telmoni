// An evaluator's, a rule's and a model connection's limits, as the server
// takes them: the forms and Server Actions check with these numbers.

/** The longest name an evaluator goes by. */
export const EVALUATOR_NAME_MAX = 60;
/** The most messages a prompt holds. */
export const EVALUATOR_MESSAGES_MAX = 20;
/** The longest message of a prompt, or a typed evaluator's state. */
export const EVALUATOR_MESSAGE_MAX = 20_000;
/** The longest sentence saying what a level, an answer or an option means. */
export const EVALUATOR_MEANS_MAX = 500;
/** The highest temperature an evaluator asks its model for. */
export const EVALUATOR_TEMPERATURE_MAX = 2;
/** The highest weekly spend limit, in dollars. */
export const EVALUATOR_WEEKLY_LIMIT_MAX = 1_000_000;

/** The most questions a typed evaluator asks. */
export const TYPED_QUESTIONS_MAX = 16;
/** The most levels a levels question holds. */
export const TYPED_LEVELS_MAX = 10;
/** The most options a choice question holds. */
export const TYPED_OPTIONS_MAX = 255;
/** The longest a question's instructions run. */
export const TYPED_INSTRUCTIONS_MAX = 4_000;

/** The longest name a rule goes by. */
export const EVALUATION_RULE_NAME_MAX = 60;
/** The most evaluators one rule runs. */
export const EVALUATION_RULE_EVALUATORS_MAX = 10;
/** The longest JSON Pointer a rule narrows a variable by. */
export const EVALUATION_RULE_POINTER_MAX = 200;

/** The longest name a model connection goes by. */
export const MODEL_CONNECTION_NAME_MAX = 60;
/** The longest model name a connection offers. */
export const MODEL_CONNECTION_MODEL_MAX = 128;
/** The most models one connection offers. */
export const MODEL_CONNECTION_MODELS_MAX = 20;
/** The longest key a connection keeps. */
export const MODEL_CONNECTION_SECRET_MAX = 16_384;
