import type { ReactNode } from "react";
import { Webhook } from "lucide-react";

import { SlackIcon } from "@/components/brand-icons";
import { LocalTime } from "@/components/local-time";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  type ConnectRefusal,
  PROVIDERS,
  PROVIDER_LABEL,
  connectOutcome,
  connectStartPath,
  connectionName,
  isOAuthProvider,
  type Provider,
} from "@/lib/connect";
import { Flag, FlagOffDetail } from "@/lib/flags";
import type { SearchParams } from "@/lib/list-query";
import { eventKindLabel } from "@/lib/notification-kinds";
import {
  fetchConnectors,
  type Connection,
} from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { administersProject } from "@/lib/project-role";

import { DeliveriesButton } from "./_deliveries";
import { ConnectionActions } from "./_manage";
import { ConnectWebhookButton } from "./_webhook";

export const metadata = { title: "Connectors" };

// Two destinations for a project's notices. Slack is an OAuth grant: an
// owner or admin is sent to the vendor, picks a channel in the vendor's own
// dialog, and the service keeps what comes back — sealed, and never shown
// here; the handshake itself is `app/connect/[provider]`. The webhook is an
// endpoint an owner or admin types into a dialog on this page, and the one
// thing shown of it afterwards is its host. This page lists what is connected
// and offers the control that connects more.
const BLURB: Record<Provider, string> = {
  slack: "Post this project's notices to a channel in your Slack workspace.",
  webhook: "Post this project's notices as signed JSON to an HTTPS endpoint you run.",
};

const ICON: Record<Provider, (props: { className?: string }) => ReactNode> = {
  slack: SlackIcon,
  webhook: Webhook,
};

// What the handshake routes send the browser back with, in a sentence.
const NOTICES: Record<ConnectRefusal, string> = {
  denied: "Nothing was connected — the request was cancelled at the vendor's page.",
  vendor: "The vendor refused the connection. Try again.",
  callback: "That connection attempt expired or was already used. Start again.",
  forbidden: "Only a project owner or admin can connect a channel.",
  off: FlagOffDetail[Flag.Connectors],
  unavailable: "Connectors are unavailable right now. Try again shortly.",
  project: "Couldn't resolve this project. Reload and try again.",
  authorize: "The connection couldn't be started. Try again.",
};

export default async function ConnectorsPage({
  params,
  searchParams,
}: {
  params: Promise<{ project: string }>;
  searchParams?: Promise<SearchParams>;
}) {
  const { project: slug } = await params;
  const outcome = connectOutcome((await searchParams) ?? {});

  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { project } = resolved;
  const projectId = project.id;
  const listing = await fetchConnectors(projectId);

  const canManage = administersProject(project.role);
  const notice =
    outcome === null
      ? null
      : "connected" in outcome
        ? `${PROVIDER_LABEL[outcome.connected]} is connected. A first message is on its way to the channel.`
        : NOTICES[outcome.error];

  return (
    <>
      <PageHeader title="Connectors" />
      <div className="grid gap-6">
        <section className="grid gap-3">
          <p className="text-sm text-muted-foreground">
            Connect this project to the places it already talks. A Slack channel
            gets every notice this project and its organization raise; a webhook,
            those among the events you choose for it. An alert&rsquo;s incidents,
            though, go only to the connectors each alert links, a webhook among
            them only if it takes their kind.
          </p>

          {notice && (
            <p
              role="status"
              className={
                outcome && "connected" in outcome
                  ? "text-sm text-brand-positive"
                  : "text-sm text-destructive"
              }
            >
              {notice}
            </p>
          )}

          {/* Two to a row from `sm` up, one below it: a tile holds a mark, a
              name and a sentence, and two of those across a phone is a
              column of initials. */}
          <div className="grid gap-3 sm:grid-cols-2">
            {PROVIDERS.map((provider) => (
              <Tile
                key={provider}
                provider={provider}
                projectId={projectId}
                canManage={canManage}
                // No listing — the service is unreachable, or not configured —
                // renders the tile exactly as it did before the lane existed.
                enabled={listing?.enabled[provider] ?? false}
                connections={(listing?.connections ?? []).filter(
                  (c) => c.provider === provider,
                )}
              />
            ))}
          </div>
        </section>
      </div>
    </>
  );
}

function Tile({
  provider,
  projectId,
  canManage,
  enabled,
  connections,
}: {
  provider: Provider;
  projectId: string;
  canManage: boolean;
  enabled: boolean;
  connections: Connection[];
}) {
  const Icon = ICON[provider];
  const name = PROVIDER_LABEL[provider];
  // `null` for the webhook: it has no handshake to start.
  const start = isOAuthProvider(provider) ? connectStartPath(provider, projectId) : null;
  return (
    <Card className="gap-4">
      <div className="flex items-start justify-between gap-3">
        <span className="flex items-center gap-3">
          {/* No colour class: these marks carry their brand's own. */}
          <Icon className="size-6 shrink-0" />
          <span className="text-sm font-medium">{name}</span>
        </span>
        {!enabled && (
          <Badge variant="secondary" className="shrink-0 font-normal">
            Soon
          </Badge>
        )}
      </div>
      <p className="text-sm text-muted-foreground">{BLURB[provider]}</p>

      {enabled && connections.length > 0 && (
        <ul className="grid gap-3">
          {connections.map((connection) => (
            <li
              key={connection.id}
              className="grid gap-2 rounded-menu border border-border p-3"
            >
              <div className="flex items-center justify-between gap-3">
                <span className="flex min-w-0 items-baseline gap-2">
                  <span className="min-w-0 truncate text-sm font-medium">
                    {connectionName(connection)}
                  </span>
                  {/* Two endpoints on one host are told apart by the start
                      of the URL's fingerprint — never by the URL. */}
                  {connection.provider === "webhook" && (
                    <span className="shrink-0 text-xs text-muted-foreground">
                      {connection.channel_id.slice(0, 8)}
                    </span>
                  )}
                </span>
                <StatusBadge status={connection.status} />
              </div>
              <p className="text-xs text-muted-foreground">
                {connection.status === "active" ? (
                  connection.last_delivery_at ? (
                    <>
                      Last message <LocalTime iso={connection.last_delivery_at} />
                    </>
                  ) : (
                    "No messages yet."
                  )
                ) : (
                  (connection.last_error ?? "Stopped.")
                )}
              </p>
              {connection.provider === "webhook" && (
                <p className="text-xs text-muted-foreground">
                  {`Events: ${eventsSummary(connection.event_kinds)}`}
                </p>
              )}
              {connection.previous_secret_expires_at && (
                <p className="text-xs text-muted-foreground">
                  Previous secret valid until{" "}
                  <LocalTime iso={connection.previous_secret_expires_at} />
                </p>
              )}
              <div className="flex flex-wrap items-center gap-3">
                <DeliveriesButton
                  projectId={projectId}
                  connectionId={connection.id}
                  label={connectionName(connection)}
                  // A webhook's receiver deduplicates a resend on its delivery
                  // id; a chat channel would show the message twice.
                  canResend={canManage && connection.provider === "webhook"}
                />
                {canManage && (
                  <ConnectionActions
                    projectId={projectId}
                    connection={connection}
                    label={connectionName(connection)}
                    reconnectHref={start}
                  />
                )}
              </div>
            </li>
          ))}
        </ul>
      )}

      {enabled ? (
        canManage ? (
          start ? (
            // A plain anchor, not `Link`: the target is a GET Route Handler
            // that STARTS a handshake, and a prefetch would start one on hover.
            <Button asChild size="sm" variant="outline" className="w-fit">
              <a href={start}>
                {connections.length > 0 ? `Connect another ${name} channel` : `Connect ${name}`}
              </a>
            </Button>
          ) : (
            <ConnectWebhookButton
              projectId={projectId}
              label={connections.length > 0 ? "Connect another endpoint" : `Connect ${name}`}
            />
          )
        ) : (
          <p className="text-xs text-muted-foreground">
            Only a project owner or admin can connect or remove a destination.
          </p>
        )
      ) : (
        <Button
          size="sm"
          variant="outline"
          disabled
          className="w-fit"
          // The name is on the button, not just beside it: a screen
          // reader moving control to control otherwise hears "Connect"
          // twice with nothing to tell the two apart.
          aria-label={`Connect ${name} — not available yet`}
        >
          Connect
        </Button>
      )}
    </Card>
  );
}

// Two labels still fit on the row's one line; past that the count says as
// much, and the Events dialog lists them.
function eventsSummary(kinds: Connection["event_kinds"]): string {
  if (kinds === null) return "All events";
  if (kinds.length > 2) return `${kinds.length} events`;
  return kinds.map(eventKindLabel).join(", ");
}

function StatusBadge({ status }: { status: Connection["status"] }) {
  if (status === "active") {
    return (
      <Badge variant="outline" className="shrink-0 font-normal">
        Connected
      </Badge>
    );
  }
  if (status === "errored") {
    return (
      <Badge variant="destructive" className="shrink-0 font-normal">
        Needs attention
      </Badge>
    );
  }
  return (
    <Badge variant="secondary" className="shrink-0 font-normal">
      Disconnected
    </Badge>
  );
}
