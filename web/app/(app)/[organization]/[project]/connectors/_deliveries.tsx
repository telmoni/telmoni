"use client";

import { unstable_rethrow } from "next/navigation";
import { Send } from "lucide-react";
import { useId, useRef, useState, useTransition } from "react";
import { toast } from "sonner";

import { readDeliveries } from "@/components/connectors/reads";
import { LocalTime } from "@/components/local-time";
import { SidePanel, SidePanelBody, SidePanelHeader, usePanelOpening } from "@/components/side-panel";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { eventKindLabel } from "@/lib/notification-kinds";
import type { DeliveryAttempt, DeliveryLogEntry } from "@/lib/server/data";
import { thrownWords } from "@/lib/use-action";

import { redeliverAction } from "./actions";

type Load =
  | { state: "loading" }
  | { state: "error"; message: string }
  | { state: "ready"; entries: DeliveryLogEntry[]; nextBefore: string | null };

// The log is a read, so it is offered to every role on the project. Resend is
// an owner's or admin's, and a webhook's only; the service refuses anything
// else anyway.
export function DeliveriesButton({
  projectId,
  connectionId,
  label,
  canResend,
}: {
  projectId: string;
  connectionId: string;
  label: string;
  canResend: boolean;
}) {
  const { opening, open, close, place } = usePanelOpening();
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [olderError, setOlderError] = useState<string | null>(null);
  const [loadingOlder, startOlder] = useTransition();
  const [resending, setResending] = useState<string | null>(null);
  // Each read is numbered so an answer that lands after a newer read was
  // started — a reopen, a resend's reload — cannot overwrite it.
  const seq = useRef(0);

  async function loadFirst() {
    const mine = ++seq.current;
    setLoad({ state: "loading" });
    setOlderError(null);
    const r = await readDeliveries(projectId, connectionId, null);
    if (mine !== seq.current) return;
    if (r.error !== undefined) {
      setLoad({ state: "error", message: r.error });
      return;
    }
    setLoad({ state: "ready", entries: r.page.deliveries, nextBefore: r.page.next_before });
  }

  function loadOlder() {
    if (load.state !== "ready" || load.nextBefore === null) return;
    const { entries, nextBefore } = load;
    const mine = seq.current;
    setOlderError(null);
    startOlder(async () => {
      const r = await readDeliveries(projectId, connectionId, nextBefore);
      if (mine !== seq.current) return;
      if (r.error !== undefined) {
        setOlderError(r.error);
        return;
      }
      setLoad({
        state: "ready",
        entries: [...entries, ...r.page.deliveries],
        nextBefore: r.page.next_before,
      });
    });
  }

  async function resend(deliveryId: string) {
    setResending(deliveryId);
    try {
      const r = await redeliverAction(projectId, connectionId, deliveryId);
      if (!r.ok) toast.error(r.error);
      else toast.success(`Delivered to ${label}.`);
      await loadFirst();
    } catch (err) {
      unstable_rethrow(err);
      toast.error(thrownWords(err));
    } finally {
      setResending(null);
    }
  }

  // Drops any read still in flight: its answer is for a panel no longer showing.
  function shut() {
    seq.current++;
    close();
  }

  return (
    <>
      <Button
        variant="outline"
        size="sm"
        aria-label={`Show deliveries to ${label}`}
        onClick={(e) => {
          open(e.currentTarget, null);
          void loadFirst();
        }}
      >
        Deliveries
      </Button>
      {place(
        opening && (
          <SidePanel panel={panel} titleId={titleId} onClose={shut} returnFocus={() => opening.returnTo}>
            <SidePanelHeader icon={Send} titleId={titleId} title={`Deliveries to ${label}`} onClose={shut}>
              <p className="text-sm text-muted-foreground">
                Newest first. Each notice is kept for 30 days, with every attempt to post it.
              </p>
            </SidePanelHeader>
            <SidePanelBody>
              <div className="py-gutter">
                {load.state === "loading" ? (
                  <p role="status" className="text-sm text-muted-foreground">
                    Loading deliveries…
                  </p>
                ) : load.state === "error" ? (
                  <div className="grid gap-2">
                    <p role="alert" className="text-sm text-destructive">
                      {load.message}
                    </p>
                    <Button
                      variant="outline"
                      size="sm"
                      className="w-fit text-muted-foreground hover:text-foreground"
                      onClick={() => void loadFirst()}
                    >
                      Try again
                    </Button>
                  </div>
                ) : load.entries.length === 0 ? (
                  <p role="status" className="text-sm text-muted-foreground">
                    No deliveries yet.
                  </p>
                ) : (
                  <div className="grid gap-gutter">
                    <ul className="grid gap-gutter">
                      {load.entries.map((entry) => (
                        <DeliveryRow
                          key={entry.id}
                          entry={entry}
                          canResend={canResend}
                          resending={resending === entry.id}
                          busy={resending !== null}
                          onResend={() => void resend(entry.id)}
                        />
                      ))}
                    </ul>
                    {olderError && (
                      <p role="alert" className="text-sm text-destructive">
                        {olderError}
                      </p>
                    )}
                    {load.nextBefore !== null && (
                      <Button
                        variant="outline"
                        size="sm"
                        className="w-fit text-muted-foreground hover:text-foreground"
                        disabled={loadingOlder}
                        onClick={loadOlder}
                      >
                        {loadingOlder ? "Loading…" : "Load older"}
                      </Button>
                    )}
                  </div>
                )}
              </div>
            </SidePanelBody>
          </SidePanel>
        ),
      )}
    </>
  );
}

function DeliveryRow({
  entry,
  canResend,
  resending,
  busy,
  onResend,
}: {
  entry: DeliveryLogEntry;
  canResend: boolean;
  resending: boolean;
  busy: boolean;
  onResend: () => void;
}) {
  return (
    <li className="grid gap-2 rounded-menu border border-border p-gutter">
      <div className="flex items-start justify-between gap-gutter">
        <span className="grid min-w-0 gap-0.5">
          <span className="text-xs text-muted-foreground">{eventKindLabel(entry.kind)}</span>
          <span className="min-w-0 truncate text-sm font-medium">{entry.subject}</span>
        </span>
        <DeliveryStatusBadge status={entry.status} attempted={entry.attempts.length > 0} />
      </div>
      <p className="text-xs text-muted-foreground">
        <LocalTime iso={entry.created_at} />
        {entry.status === "pending" && (
          <>
            {" · "}Next attempt <LocalTime iso={entry.next_attempt_at} />
          </>
        )}
      </p>
      {entry.status !== "delivered" && entry.last_error && (
        <p className="text-xs wrap-break-word text-destructive">{entry.last_error}</p>
      )}
      <details className="text-xs">
        <summary className="w-fit cursor-pointer text-muted-foreground">Details</summary>
        <pre className="mt-2 max-h-48 overflow-auto rounded-md bg-muted px-gutter py-2 text-xs whitespace-pre-wrap wrap-break-word">
          {entry.body}
        </pre>
        {entry.attempts.length === 0 ? (
          <p className="mt-2 text-muted-foreground">No attempts yet.</p>
        ) : (
          <ol className="mt-2 grid gap-2">
            {entry.attempts.map((attempt) => (
              <AttemptRow key={attempt.id} attempt={attempt} />
            ))}
          </ol>
        )}
      </details>
      {canResend && (
        // The same delivery id goes out again, so a receiver that already
        // has this notice drops the copy on its `Telmoni-Delivery-Id`.
        <Button
          variant="outline"
          size="sm"
          className="w-fit text-muted-foreground hover:text-foreground"
          disabled={busy}
          aria-label={`Resend ${entry.subject}`}
          onClick={onResend}
        >
          {resending ? "Resending…" : "Resend"}
        </Button>
      )}
    </li>
  );
}

function AttemptRow({ attempt }: { attempt: DeliveryAttempt }) {
  return (
    <li className="grid gap-0.5 border-l border-border pl-2">
      <span>
        <LocalTime iso={attempt.created_at} />
        {" · "}
        {attempt.trigger === "manual" ? "Manual resend" : "Scheduled"}
        {" · "}
        <span className={attempt.outcome === "failed" ? "text-destructive" : undefined}>
          {attempt.outcome === "delivered" ? "Delivered" : "Failed"}
        </span>
        {" · "}
        {attempt.status_code === null ? "No response" : `HTTP ${attempt.status_code}`}
        {" · "}
        {attempt.duration_ms} ms
      </span>
      {attempt.error && (
        <span className="wrap-break-word text-muted-foreground">{attempt.error}</span>
      )}
    </li>
  );
}

// A pending row nobody has sent yet is queued, not retrying: the difference
// is whether the receiver has already refused it once.
function DeliveryStatusBadge({
  status,
  attempted,
}: {
  status: DeliveryLogEntry["status"];
  attempted: boolean;
}) {
  if (status === "delivered") {
    return (
      <Badge variant="outline">
        Delivered
      </Badge>
    );
  }
  if (status === "failed") {
    return (
      <Badge variant="destructive">
        Failed
      </Badge>
    );
  }
  return (
    <Badge variant="secondary">
      {attempted ? "Retrying" : "Queued"}
    </Badge>
  );
}
