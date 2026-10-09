"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { Connection } from "@/lib/server/data";

import { EventPicker, choiceFrom, isChoiceEmpty, kindsFrom, type EventChoice } from "./_events";
import { SigningSecretDialog } from "./_webhook";
import {
  disconnectConnectorAction,
  rotateWebhookSecretAction,
  sendTestMessageAction,
  updateWebhookEventsAction,
  type SecretResult,
} from "./actions";

export function ConnectionActions({
  projectId,
  connection,
  label,
  reconnectHref,
}: {
  projectId: string;
  connection: Connection;
  label: string;
  // The handshake that repairs a vendor row in place; `null` for a webhook,
  // whose repair is a rotation.
  reconnectHref: string | null;
}) {
  const router = useRouter();
  const [pending, startTransition] = useTransition();
  const [rotated, setRotated] = useState<SecretResult | null>(null);
  const [rotating, setRotating] = useState(false);
  const [editingEvents, setEditingEvents] = useState(false);
  const isWebhook = connection.provider === "webhook";

  function sendTest() {
    startTransition(async () => {
      try {
        const r = await sendTestMessageAction(projectId, connection.id);
        if (r.error) toast.error(r.error);
        else toast.success(`Test message sent to ${label}.`);
        router.refresh();
      } catch {
        toast.error("Network error. Try again.");
      }
    });
  }

  function disconnect() {
    startTransition(async () => {
      try {
        const r = await disconnectConnectorAction(projectId, connection.id);
        if (r.error) toast.error(r.error);
        else {
          toast.success(`${label} disconnected.`);
          router.refresh();
        }
      } catch {
        toast.error("Network error. Try again.");
      }
    });
  }

  return (
    <div className="flex flex-wrap items-center gap-3">
      {connection.status === "active" && (
        <Button
          variant="outline"
          size="sm"
          className="h-9 text-xs"
          disabled={pending}
          onClick={sendTest}
        >
          Send test
        </Button>
      )}
      {isWebhook ? (
        <>
          <Button
            variant="outline"
            size="sm"
            className="h-9 text-xs"
            disabled={pending}
            aria-label={`Choose the events sent to ${label}`}
            onClick={() => setEditingEvents(true)}
          >
            Events
          </Button>
          <EventsDialog
            key={`events-${editingEvents}`}
            projectId={projectId}
            connection={connection}
            label={label}
            open={editingEvents}
            onOpenChange={setEditingEvents}
          />
          {/* Offered in every status: rotating is also how an errored
              endpoint comes back. */}
          <Button
            variant="outline"
            size="sm"
            className="h-9 text-xs"
            disabled={pending}
            aria-label={`Rotate the signing secret for ${label}`}
            onClick={() => setRotating(true)}
          >
            Rotate secret
          </Button>
          <RotateDialog
            key={`rotate-${rotating}`}
            projectId={projectId}
            connectionId={connection.id}
            label={label}
            open={rotating}
            onOpenChange={setRotating}
            onRotated={setRotated}
          />
        </>
      ) : (
        connection.status !== "active" &&
        reconnectHref && (
          // The same handshake as a first install: it lands on the same key
          // and repairs the row in place.
          <Button asChild variant="outline" size="sm" className="h-9 text-xs">
            <a href={reconnectHref}>Reconnect</a>
          </Button>
        )
      )}
      {rotated && <SigningSecretDialog result={rotated} onClose={() => setRotated(null)} />}
      <ConfirmDialog
        trigger={
          <Button
            variant="outline"
            size="sm"
            className="h-9 text-xs text-destructive hover:text-destructive hover:bg-destructive/10"
            disabled={pending}
            aria-label={`Disconnect ${label}`}
          >
            Disconnect
          </Button>
        }
        title={`Disconnect ${label}?`}
        description={
          isWebhook
            ? "Notices stop posting there immediately. Anything already queued for it is dropped, and its signing secret stops verifying. You can connect the endpoint again at any time, with a new secret."
            : "Notices stop posting there immediately. Anything already queued for it is dropped. You can connect it again at any time."
        }
        confirmLabel="Disconnect"
        destructive
        onConfirm={disconnect}
      />
    </div>
  );
}

function EventsDialog({
  projectId,
  connection,
  label,
  open,
  onOpenChange,
}: {
  projectId: string;
  connection: Connection;
  label: string;
  open: boolean;
  onOpenChange: (v: boolean) => void;
}) {
  const router = useRouter();
  const [events, setEvents] = useState<EventChoice>(() => choiceFrom(connection.event_kinds));
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  const canSubmit = !isChoiceEmpty(events) && !pending;

  function submit() {
    setError(null);
    start(async () => {
      try {
        const r = await updateWebhookEventsAction(projectId, connection.id, kindsFrom(events));
        if (r.error !== null) {
          setError(r.error);
          return;
        }
        onOpenChange(false);
        toast.success(`Events updated for ${label}.`);
        router.refresh();
      } catch {
        setError("Network error. Try again.");
      }
    });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <form
          className="contents"
          onSubmit={(e) => {
            e.preventDefault();
            if (canSubmit) submit();
          }}
        >
          <DialogHeader>
            <DialogTitle>Events for {label}</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <DialogDescription>
              Only the chosen events are posted to this endpoint. A change applies to
              notices raised from now on.
            </DialogDescription>
            <EventPicker
              value={events}
              onChange={(next) => {
                setEvents(next);
                setError(null);
              }}
              disabled={pending}
            />
            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
          </DialogBody>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!canSubmit}>
              {pending ? "Saving…" : "Save"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// The overlaps the service accepts span 0 to 24 hours; these are the three
// worth offering: none, for a leaked secret, and two sizes of deploy window.
const OVERLAP_HOURS = ["24", "1", "0"] as const;
type OverlapHours = (typeof OVERLAP_HOURS)[number];

const OVERLAP_LABEL: Record<OverlapHours, string> = {
  "24": "Keep for 24 hours",
  "1": "Keep for 1 hour",
  "0": "Stop immediately",
};

function isOverlapHours(value: string): value is OverlapHours {
  return (OVERLAP_HOURS as readonly string[]).includes(value);
}

function RotateDialog({
  projectId,
  connectionId,
  label,
  open,
  onOpenChange,
  onRotated,
}: {
  projectId: string;
  connectionId: string;
  label: string;
  open: boolean;
  onOpenChange: (v: boolean) => void;
  onRotated: (r: SecretResult) => void;
}) {
  const router = useRouter();
  const [hours, setHours] = useState<OverlapHours>("24");
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  function submit() {
    setError(null);
    start(async () => {
      try {
        const r = await rotateWebhookSecretAction(projectId, connectionId, Number(hours));
        if (r.error !== null) {
          setError(r.error);
          return;
        }
        onOpenChange(false);
        onRotated(r);
        router.refresh();
      } catch {
        setError("Network error. Try again.");
      }
    });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <form
          className="contents"
          onSubmit={(e) => {
            e.preventDefault();
            if (!pending) submit();
          }}
        >
          <DialogHeader>
            <DialogTitle>Rotate the signing secret for {label}?</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <DialogDescription>
              The new secret is shown once, for you to put in your receiver. Until the
              current secret stops, each delivery carries two signatures, one per secret,
              so a receiver that verifies either keeps working. Stop it immediately if it
              has leaked. If this endpoint had stopped, rotating brings it back.
            </DialogDescription>
            <div className="grid gap-2">
              <Label htmlFor="rotate-overlap">Current secret</Label>
              <Select
                value={hours}
                onValueChange={(v) => {
                  if (isOverlapHours(v)) setHours(v);
                }}
                disabled={pending}
              >
                <SelectTrigger id="rotate-overlap" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {OVERLAP_HOURS.map((h) => (
                    <SelectItem key={h} value={h}>
                      {OVERLAP_LABEL[h]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
          </DialogBody>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={pending}>
              {pending ? "Rotating…" : "Rotate"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
