import { isOneOf } from "@/lib/one-of";
import { ApiKeyScope } from "@/lib/types/enums";

/** A scope as the key list names it. */
export const SCOPE_LABEL: Record<ApiKeyScope, string> = {
  [ApiKeyScope.Ingest]: "Send spans",
  [ApiKeyScope.Read]: "Read the API",
  [ApiKeyScope.Evaluate]: "Change evaluators",
};

const SCOPE_DETAIL: Record<ApiKeyScope, string> = {
  [ApiKeyScope.Ingest]: "OpenTelemetry exports to /otlp/v1/traces.",
  [ApiKeyScope.Read]: "The /v1 lanes, which telmoni status reads.",
  [ApiKeyScope.Evaluate]: "Making, versioning and deleting evaluators under /v1/evaluators, for a key that reads too.",
};

/** What each scope opens, in the order the console offers them: sending
 *  first, since that is what most keys are minted for. */
export const API_KEY_SCOPES: readonly { scope: ApiKeyScope; label: string; detail: string }[] = [
  ApiKeyScope.Ingest,
  ApiKeyScope.Read,
  ApiKeyScope.Evaluate,
].map((scope) => ({ scope, label: SCOPE_LABEL[scope], detail: SCOPE_DETAIL[scope] }));

/** Whether `value` is a scope the server knows. */
export function isApiKeyScope(value: string): value is ApiKeyScope {
  return isOneOf(Object.values(ApiKeyScope), value);
}

/** The scopes a new key starts with: what most keys are minted for, never a
 *  change of the project's evaluators, which is asked for by name. */
export const DEFAULT_SCOPES: readonly ApiKeyScope[] = [ApiKeyScope.Ingest, ApiKeyScope.Read];

/** `scopes` with `scope` ticked or not: changing evaluators reads too, so
 *  ticking it ticks reading, and unticking reading unticks it. */
export function withScope(scopes: readonly ApiKeyScope[], scope: ApiKeyScope, on: boolean): ApiKeyScope[] {
  const kept = new Set(scopes);
  if (on) {
    kept.add(scope);
    if (scope === ApiKeyScope.Evaluate) kept.add(ApiKeyScope.Read);
  } else {
    kept.delete(scope);
    if (scope === ApiKeyScope.Read) kept.delete(ApiKeyScope.Evaluate);
  }
  return API_KEY_SCOPES.map((each) => each.scope).filter((each) => kept.has(each));
}
