"use client";

import { useRouter } from "next/navigation";
import { useRef, useState, useTransition } from "react";
import { toast } from "sonner";

import { LocalTime } from "@/components/local-time";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { eventKindLabel } from "@/lib/notification-kinds";
import type { DeliveryAttempt, DeliveryLogEntry } from "@/lib/server/data";

import { listDeliveriesAction, redeliverAction } from "./actions";

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
  const router = useRouter();
  const [open, setOpen] = useState(false);
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
    try {
      const r = await listDeliveriesAction(projectId, connectionId, null);
      if (mine !== seq.current) return;
      if (r.error !== null || !r.page) {
        setLoad({ state: "error", message: r.error ?? "Couldn't load the deliveries." });
        return;
      }
      setLoad({ state: "ready", entries: r.page.deliveries, nextBefore: r.page.next_before });
    } catch {
      if (mine === seq.current) setLoad({ state: "error", message: "Network error. Try again." });
    }
  }

  function loadOlder() {
    if (load.state !== "ready" || load.nextBefore === null) return;
    const { entries, nextBefore } = load;
    const mine = seq.current;
    setOlderError(null);
    startOlder(async () => {
      try {
        const r = await listDeliveriesAction(projectId, connectionId, nextBefore);
        if (mine !== seq.current) return;
        if (r.error !== null || !r.page) {
          setOlderError(r.error ?? "Couldn't load older deliveries.");
          return;
        }
        setLoad({
          state: "ready",
          entries: [...entries, ...r.page.deliveries],
          nextBefore: r.page.next_before,
        });
      } catch {
        if (mine === seq.current) setOlderError("Network error. Try again.");
      }
    });
  }

  async function resend(deliveryId: string) {
    setResending(deliveryId);
    try {
      const r = await redeliverAction(projectId, connectionId, deliveryId);
      if (r.error) toast.error(r.error);
      else toast.success(`Delivered to ${label}.`);
      router.refresh();
      await loadFirst();
    } catch {
      toast.error("Network error. Try again.");
    } finally {
      setResending(null);
    }
  }

  return (
    <>
      <Button
        variant="outline"
        size="sm"
        className="h-9 text-xs"
        aria-label={`Show deliveries to ${label}`}
        onClick={() => {
          setOpen(true);
          void loadFirst();
        }}
      >
        Deliveries
      </Button>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          // Drops any read still in flight: its answer is for a dialog that
          // is no longer showing.
          if (!next) seq.current++;
        }}
      >
        <DialogContent className="sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>Deliveries to {label}</DialogTitle>
          </DialogHeader>
          {/* The body is the scrolling part of the chassis, so the list needs
              no scroll container of its own. */}
          <DialogBody>
            <DialogDescription>
              Newest first. Each notice is kept for 30 days, with every attempt to post it.
            </DialogDescription>
            {load.state === "loading" ? (
              <p role="status" className="py-2 text-sm text-muted-foreground">
                Loading deliveries…
              </p>
            ) : load.state === "error" ? (
              <div className="grid gap-2 py-2">
                <p role="alert" className="text-sm text-destructive">
                  {load.message}
                </p>
                <Button
                  variant="outline"
                  size="sm"
                  className="h-9 w-fit text-xs"
                  onClick={() => void loadFirst()}
                >
                  Try again
                </Button>
              </div>
            ) : load.entries.length === 0 ? (
              <p role="status" className="py-2 text-sm text-muted-foreground">
                No deliveries yet.
              </p>
            ) : (
              <div className="grid gap-3">
                <ul className="grid gap-3">
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
                    className="h-9 w-fit text-xs"
                    disabled={loadingOlder}
                    onClick={loadOlder}
                  >
                    {loadingOlder ? "Loading…" : "Load older"}
                  </Button>
                )}
              </div>
            )}
          </DialogBody>
          {/* Nothing to act on here, and the footer stays: every modal keeps
              its way out where the others keep theirs. */}
          <DialogFooter>
            <DialogClose asChild>
              <Button>Done</Button>
            </DialogClose>
          </DialogFooter>
        </DialogContent>
      </Dialog>
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
    <li className="grid gap-2 rounded-menu border border-border p-3">
      <div className="flex items-start justify-between gap-3">
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
        <p className="text-xs wrap-break-words text-destructive">{entry.last_error}</p>
      )}
      <details className="text-xs">
        <summary className="w-fit cursor-pointer text-muted-foreground">Details</summary>
        <pre className="mt-2 max-h-48 overflow-auto rounded-menu bg-muted p-2 font-mono text-xs whitespace-pre-wrap wrap-break-words">
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
          className="h-9 w-fit text-xs"
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
        <span className="wrap-break-words text-muted-foreground">{attempt.error}</span>
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
      <Badge variant="outline" className="shrink-0 font-normal">
        Delivered
      </Badge>
    );
  }
  if (status === "failed") {
    return (
      <Badge variant="destructive" className="shrink-0 font-normal">
        Failed
      </Badge>
    );
  }
  return (
    <Badge variant="secondary" className="shrink-0 font-normal">
      {attempted ? "Retrying" : "Queued"}
    </Badge>
  );
}
