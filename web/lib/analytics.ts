import { fetchWithTimeout } from "./api/fetch";
import { env } from "./env";
import { logger } from "./logger";

type AnalyticsEvent =
  | "onboarding.signup_completed"
  | "activation.topup_started";

// Whether this deployment has a sink at all. Without one nothing is sent
// whatever the person chooses, so the console offers no switch: a consent for
// nothing would promise a provider that does not exist.
export function analyticsConfigured(): boolean {
  return Boolean(env.ANALYTICS_INGEST_URL && env.ANALYTICS_WRITE_KEY);
}

export function track(
  event: AnalyticsEvent,
  props: Record<string, unknown> = {},
  { consented = false }: { consented?: boolean } = {},
): void {
  try {
    logger.info({ analytics: true, event, ...props }, event);
    if (!consented) return;
    void send(event, props).catch(() => {});
  } catch {
  }
}

async function send(
  event: AnalyticsEvent,
  props: Record<string, unknown>,
): Promise<void> {
  const url = env.ANALYTICS_INGEST_URL;
  const key = env.ANALYTICS_WRITE_KEY;
  if (!url || !key) return;

  const distinctId =
    typeof props.userId === "string" && props.userId
      ? props.userId
      : "anonymous";

  // Through the wrapper like every other outbound call, with the short
  // deadline an ingest deserves: nobody is waiting on this, and a stalled
  // provider should cost three seconds of a background promise, not more.
  await fetchWithTimeout(
    url,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        api_key: key,
        event,
        distinct_id: distinctId,
        properties: props,
      }),
      keepalive: true,
    },
    3_000,
  );
}
