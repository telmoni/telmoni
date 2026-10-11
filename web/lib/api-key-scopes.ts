import { ApiKeyScope } from "@/lib/types/enums";

/** What each scope opens, in the order the console offers them: sending
 *  first, since that is what most keys are minted for. */
export const API_KEY_SCOPES: readonly { scope: ApiKeyScope; label: string; detail: string }[] = [
  {
    scope: ApiKeyScope.Ingest,
    label: "Send spans",
    detail: "OpenTelemetry exports to /otlp/v1/traces.",
  },
  {
    scope: ApiKeyScope.Read,
    label: "Read the API",
    detail: "The /v1 lanes, which telmoni status reads.",
  },
];

/** A scope as the key list names it. */
export const SCOPE_LABEL = Object.fromEntries(
  API_KEY_SCOPES.map(({ scope, label }) => [scope, label]),
) as Record<ApiKeyScope, string>;

/** Whether `value` is a scope the server knows. */
export function isApiKeyScope(value: string): value is ApiKeyScope {
  return API_KEY_SCOPES.some(({ scope }) => scope === value);
}
