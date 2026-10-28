import { SESSION_EXPIRED } from "@/lib/action-result";
import { problemWords } from "@/lib/api/problem";
import type { DeliveryLogPage } from "@/lib/server/data";

// A page of a connection's delivery log, from its Route Handler
// (`app/api/v1/connectors/[connection]/deliveries`), made in the browser and so
// kept out of `app/`, where a bare fetch is a server's. A refusal comes back
// as the sentence the dialog shows: the handler's own, or — for a lapsed
// session the console's proxy turned away on `/api/*` before any handler ran
// — the one the handler would have given. A network failure throws, as a
// fetch does.
export async function readDeliveries(
  projectId: string,
  connectionId: string,
  before: string | null,
): Promise<{ page: DeliveryLogPage; error?: undefined } | { error: string }> {
  const query = new URLSearchParams({ project: projectId });
  if (before !== null) query.set("before", before);
  const res = await fetch(
    `/api/v1/connectors/${encodeURIComponent(connectionId)}/deliveries?${query}`,
  );
  const body: unknown = await res.json().catch(() => null);
  if (res.ok && body && typeof body === "object") return { page: body as DeliveryLogPage };
  if (res.status === 401) return { error: SESSION_EXPIRED };
  return {
    error:
      problemWords(body) ||
      (before === null ? "Couldn't load the deliveries." : "Couldn't load older deliveries."),
  };
}
