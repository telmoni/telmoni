import { isOneOf } from "@/lib/one-of";
import { ApiKeyScope } from "@/lib/types/enums";

/** A scope as the key list names it. */
export const SCOPE_LABEL: Record<ApiKeyScope, string> = {
  [ApiKeyScope.Ingest]: "Send spans",
  [ApiKeyScope.Read]: "Read the API",
};

const SCOPE_DETAIL: Record<ApiKeyScope, string> = {
  [ApiKeyScope.Ingest]: "OpenTelemetry exports to /otlp/v1/traces.",
  [ApiKeyScope.Read]: "The /v1 lanes, which telmoni status reads.",
};

/** What each scope opens, in the order the console offers them: sending
 *  first, since that is what most keys are minted for. */
export const API_KEY_SCOPES: readonly { scope: ApiKeyScope; label: string; detail: string }[] = [
  ApiKeyScope.Ingest,
  ApiKeyScope.Read,
].map((scope) => ({ scope, label: SCOPE_LABEL[scope], detail: SCOPE_DETAIL[scope] }));

/** Whether `value` is a scope the server knows. */
export function isApiKeyScope(value: string): value is ApiKeyScope {
  return isOneOf(Object.values(ApiKeyScope), value);
}
