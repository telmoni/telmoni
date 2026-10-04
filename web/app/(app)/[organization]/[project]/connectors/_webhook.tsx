"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition, type ReactNode } from "react";

import { PlaintextTokenDialog } from "@/components/plaintext-token-dialog";
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { DOCS_URL } from "@/lib/site";

import { EventPicker, isChoiceEmpty, kindsFrom, type EventChoice } from "./_events";
import { createWebhookConnectorAction, type SecretResult } from "./actions";

// The webhook is the one connector with no handshake: an owner or admin types the
// endpoint here, the service checks it and mints the signing secret, and the
// secret is shown exactly once on the way back. Slack and Discord are a link
// to a GET route instead, because the browser has to leave for the vendor.
export function ConnectWebhookButton({ projectId, label }: { projectId: string; label: string }) {
  const [open, setOpen] = useState(false);
  const [minted, setMinted] = useState<SecretResult | null>(null);

  return (
    <>
      <Button size="sm" variant="outline" className="w-fit" onClick={() => setOpen(true)}>
        {label}
      </Button>
      <AddEndpointDialog
        key={`add-${open}`}
        projectId={projectId}
        open={open}
        onOpenChange={setOpen}
        onConnected={setMinted}
      />
      {minted?.signingSecret && (
        <SigningSecretDialog result={minted} onClose={() => setMinted(null)} />
      )}
    </>
  );
}

// Shown once, for a new endpoint and for a rotation alike. The description
// is the whole integration contract, because this is the one moment the
// person is certainly reading.
export function SigningSecretDialog({
  result,
  onClose,
}: {
  result: SecretResult;
  onClose: () => void;
}) {
  if (!result.signingSecret) return null;
  return (
    <PlaintextTokenDialog
      token={result.signingSecret}
      title={result.host ? `Signing secret for ${result.host}` : "Signing secret"}
      description={<SecretDescription />}
      closeLabel="I've saved it"
      onClose={onClose}
    />
  );
}

// The receiver's page on the docs site. One address, so the dialog and the
// docs cannot disagree about where the contract is written down.
const WEBHOOK_DOCS_URL = `${DOCS_URL}/integrations/webhooks/`;

function SecretDescription(): ReactNode {
  return (
    <>
      Copy this secret now: <strong>it will not be shown again</strong>. Every delivery
      carries a <code>Telmoni-Signature</code> header, <code>t=&lt;unix&gt;,v1=&lt;hex&gt;</code>,
      an HMAC-SHA256 over <code>{"{t}.{body}"}</code> with this secret, and a{" "}
      <code>Telmoni-Delivery-Id</code> that is stable across retries of one notice.
      While a replaced secret still works the header carries a second <code>v1</code>,
      so accept a delivery when any <code>v1</code> matches.
      The receiver&rsquo;s side &mdash; verifying, the replay window and retries &mdash; is
      at{" "}
      <a href={WEBHOOK_DOCS_URL} target="_blank" rel="noreferrer" className="underline">
        {WEBHOOK_DOCS_URL}
      </a>
      .
    </>
  );
}

function AddEndpointDialog({
  projectId,
  open,
  onOpenChange,
  onConnected,
}: {
  projectId: string;
  open: boolean;
  onOpenChange: (v: boolean) => void;
  onConnected: (r: SecretResult) => void;
}) {
  const router = useRouter();
  const [url, setUrl] = useState("");
  const [events, setEvents] = useState<EventChoice>({ all: true, kinds: [] });
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  const canSubmit = url.trim().length > 0 && !isChoiceEmpty(events) && !pending;

  function submit() {
    setError(null);
    start(async () => {
      try {
        const r = await createWebhookConnectorAction(projectId, url, kindsFrom(events));
        if (r.error !== null) {
          setError(r.error);
          return;
        }
        onOpenChange(false);
        onConnected(r);
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
            <DialogTitle>Connect a webhook</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <DialogDescription>
              HTTPS only, and it must resolve to a public address. The events you choose
              are posted there as signed JSON; the signing secret is shown exactly once,
              after the endpoint is checked.
            </DialogDescription>
            <div className="grid gap-2">
              <Label htmlFor="webhook-url">Endpoint URL</Label>
              <Input
                id="webhook-url"
                type="url"
                value={url}
                onChange={(e) => {
                  setUrl(e.target.value);
                  setError(null);
                }}
                placeholder="https://example.com/telmoni/events"
                spellCheck={false}
                autoComplete="off"
                maxLength={2048}
                required
                disabled={pending}
              />
            </div>
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
              {pending ? "Connecting…" : "Connect"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
