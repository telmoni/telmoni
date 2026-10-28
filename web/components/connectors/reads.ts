import { readRoute } from "@/components/route-read";
import { refusalWords } from "@/lib/route-read";
import { DeliveryLogPageSchema, type DeliveryLogPage } from "@/lib/types/connectors";

// A page of a connection's delivery log, from its Route Handler
// (`app/api/v1/connectors/[connection]/deliveries`), made in the browser and so
// kept out of `app/`, where a bare fetch is a server's. A refusal comes back
// as the sentence the dialog shows: the handler's own, or — for a lapsed
// session the console's proxy turned away on `/api/*` before any handler ran
// — the one the handler would have given.
export async function readDeliveries(
  projectId: string,
  connectionId: string,
  before: string | null,
): Promise<{ page: DeliveryLogPage; error?: undefined } | { error: string }> {
  const query = new URLSearchParams({ project: projectId });
  if (before !== null) query.set("before", before);
  const read = await readRoute(
    `/api/v1/connectors/${encodeURIComponent(connectionId)}/deliveries?${query}`,
    DeliveryLogPageSchema,
  );
  if (read.kind === "ok") return { page: read.value };
  return {
    error: refusalWords(
      read,
      before === null ? "Couldn't load the deliveries." : "Couldn't load older deliveries.",
    ),
  };
}
