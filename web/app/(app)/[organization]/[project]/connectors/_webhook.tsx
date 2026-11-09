"use client";

import { Webhook } from "lucide-react";
import { startTransition, useId, useRef, useState, type ReactNode } from "react";

import { PlaintextTokenDialog } from "@/components/plaintext-token-dialog";
import {
  SidePanel,
  SidePanelBody,
  SidePanelFooter,
  SidePanelHeader,
  SidePanelRow,
  usePanelOpening,
} from "@/components/side-panel";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { DOCS_URL } from "@/lib/site";
import { useAction } from "@/lib/use-action";

import { EventPicker, isChoiceEmpty, kindsFrom, type EventChoice } from "./_events";
import { createWebhookConnectorAction, type SigningSecret } from "./actions";

// The webhook is the one connector with no handshake: an owner or admin types the
// endpoint here, the service checks it and mints the signing secret, and the
// secret is shown exactly once on the way back. Slack is a link to a GET
// route instead, because the browser has to leave for the vendor.
export function ConnectWebhookButton({ projectId, label }: { projectId: string; label: ReactNode }) {
  const { opening, open, close, place } = usePanelOpening();
  const [minted, setMinted] = useState<SigningSecret | null>(null);

  return (
    <>
      <Button size="sm" variant="filled" className="w-fit" onClick={(e) => open(e.currentTarget, null)}>
        {label}
      </Button>
      {place(
        opening && (
          <AddEndpointPanel
            key={opening.at}
            projectId={projectId}
            onClose={close}
            returnFocus={() => opening.returnTo}
            onConnected={(r) => {
              close();
              setMinted(r);
            }}
          />
        ),
      )}
      {minted && <SigningSecretDialog result={minted} onClose={() => setMinted(null)} />}
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
  result: SigningSecret;
  onClose: () => void;
}) {
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
const WEBHOOK_DOCS_URL = `${DOCS_URL}/integrations/webhooks`;

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
        the webhooks guide
      </a>
      .
    </>
  );
}

function AddEndpointPanel({
  projectId,
  onClose,
  returnFocus,
  onConnected,
}: {
  projectId: string;
  onClose: () => void;
  returnFocus: () => HTMLElement | null;
  onConnected: (r: SigningSecret) => void;
}) {
  const titleId = useId();
  const eventsId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const [url, setUrl] = useState("");
  const [name, setName] = useState("");
  const [events, setEvents] = useState<EventChoice>({ all: true, kinds: [] });
  const { pending, error, setError, run } = useAction();

  const canSubmit = url.trim().length > 0 && !isChoiceEmpty(events) && !pending;

  function submit() {
    run(
      () => createWebhookConnectorAction(projectId, url, kindsFrom(events), name),
      (r) =>
        // In the transition, with the list the action drew again: after the
        // `await` it would be an update of its own, a frame ahead of it.
        startTransition(() => onConnected(r)),
    );
  }

  return (
    <SidePanel panel={panel} titleId={titleId} onClose={onClose} full initialFocus={field} returnFocus={returnFocus}>
      <form
        className="flex min-h-0 flex-1 flex-col"
        onSubmit={(e) => {
          e.preventDefault();
          if (canSubmit) submit();
        }}
      >
        <SidePanelHeader icon={Webhook} titleId={titleId} title="Connect a webhook" onClose={onClose}>
          <p className="text-sm text-muted-foreground">
            HTTPS only, and it must resolve to a public address. The events you choose
            are posted there as signed JSON; the signing secret is shown exactly once,
            after the endpoint is checked.
          </p>
        </SidePanelHeader>
        <SidePanelBody>
          <SidePanelRow label="Endpoint URL" htmlFor="webhook-url">
            <Input
              ref={field}
              id="webhook-url"
              type="url"
              className="max-w-md"
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
          </SidePanelRow>
          <SidePanelRow label="Name (optional)" htmlFor="webhook-name">
            <Input
              id="webhook-name"
              className="max-w-md"
              value={name}
              onChange={(e) => {
                setName(e.target.value);
                setError(null);
              }}
              placeholder="e.g. On-call bot"
              autoComplete="off"
              maxLength={100}
              disabled={pending}
            />
          </SidePanelRow>
          <SidePanelRow label="Events" htmlFor={eventsId}>
            <EventPicker
              id={eventsId}
              value={events}
              onChange={(next) => {
                setEvents(next);
                setError(null);
              }}
              disabled={pending}
            />
          </SidePanelRow>
          {error && (
            <p className="py-2 text-sm text-destructive" role="alert">
              {error}
            </p>
          )}
        </SidePanelBody>
        <SidePanelFooter>
          <Button type="submit" variant="filled" disabled={!canSubmit}>
            {pending ? "Connecting…" : "Connect"}
          </Button>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
        </SidePanelFooter>
      </form>
    </SidePanel>
  );
}
