"use client";

import Link from "next/link";
import { startTransition, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { KeyRound } from "lucide-react";

import { useConsoleShell } from "@/components/console-shell";
import { InstallTabs } from "@/components/install-tabs";
import { PageAction } from "@/components/page-action";
import { PlaintextTokenDialog } from "@/components/plaintext-token-dialog";
import {
  SidePanel,
  SidePanelFooter,
  SidePanelHeader,
  SidePanelRow,
} from "@/components/side-panel";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { API_KEY_SCOPES } from "@/lib/api-key-scopes";
import { firstTraceSnippets } from "@/lib/first-trace";
import { MAX_API_KEY_DESCRIPTION, MAX_API_KEY_NAME, typedBound } from "@/lib/names";
import { PRODUCT_NAME } from "@/lib/site";
import { ApiKeyScope } from "@/lib/types/enums";
import { useAction } from "@/lib/use-action";

import { mintTokenAction, type MintedKey } from "./actions";

// Mint key opens in the console's side panel, full over the content area, as
// New project and New organization do (`components/create-project.tsx`); the
// key itself, shown once, stays a dialog in front of the page, since it must
// be copied before anything else happens.

/** One opening of the panel: counted, so asking again draws it afresh, and
 *  the control the focus goes back to as it shuts. */
type Opening = { at: number; returnTo: HTMLElement | null };

/** A key just minted, and whether it sends spans. */
type NewKey = MintedKey & { ingest: boolean };

export function KeysActions({
  projectId,
  tracesHref,
}: {
  /// The project this page rendered; a key is minted on it or on nothing.
  projectId: string;
  /// Its Traces page, where a key's first trace lists.
  tracesHref: string;
}) {
  const [opening, setOpening] = useState<Opening | null>(null);
  const [minted, setMinted] = useState<NewKey | null>(null);
  const openings = useRef(0);
  const { element } = useConsoleShell();


  const panel = opening && (
    <MintKeyPanel
      key={opening.at}
      projectId={projectId}
      onClose={() => setOpening(null)}
      returnFocus={() => opening.returnTo}
      onMinted={(result) => {
        setOpening(null);
        setMinted(result);
      }}
    />
  );

  return (
    <div className="flex items-center gap-1">
      <PageAction
        primary="Mint key"
        onClick={(e) => setOpening({ at: ++openings.current, returnTo: e.currentTarget })}
      >
        <KeyRound className="size-4" />
        Mint key
      </PageAction>

      {panel && (element ? createPortal(panel, element) : panel)}

      {minted && (
        <PlaintextTokenDialog
          token={minted.token}
          title="Key created"
          description={
            <>
              Copy this key now: <strong>it will not be shown again</strong>.{" "}
              {PRODUCT_NAME} never stores the plaintext.
            </>
          }
          closeLabel="I've saved it"
          onClose={() => setMinted(null)}
        >
          {minted.ingest && <FirstTrace token={minted.token} tracesHref={tracesHref} />}
        </PlaintextTokenDialog>
      )}
    </div>
  );
}

/** A key that sends spans, a paste away from its first trace: each SDK's
 *  smallest run with the key in it. */
function FirstTrace({ token, tracesHref }: { token: string; tracesHref: string }) {
  return (
    <div className="flex flex-col gap-gutter">
      <p className="text-sm text-muted-foreground">
        Send a first trace: paste one into a terminal. Its run lists on the{" "}
        {/* A tab of its own: leaving the page would take the key off the screen. */}
        <Link
          href={tracesHref}
          target="_blank"
          rel="noopener"
          className="underline underline-offset-4"
        >
          Traces
        </Link>{" "}
        page within a minute. On your own deployment, set <code>TELMONI_ENDPOINT</code> to its
        machine host first, on the line each holds for it.
      </p>
      <InstallTabs name="First run" tabs={firstTraceSnippets(token)} />
    </div>
  );
}

/** The key's name, what it is for, what it opens and when it expires, in the
 *  side panel; minted, the panel shuts and the key is shown once. */
function MintKeyPanel({
  projectId,
  onClose,
  onMinted,
  returnFocus,
}: {
  projectId: string;
  onClose: () => void;
  onMinted: (r: NewKey) => void;
  returnFocus?: () => HTMLElement | null;
}) {
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [ttl, setTtl] = useState("90d");
  const [scopes, setScopes] = useState<ApiKeyScope[]>(API_KEY_SCOPES.map(({ scope }) => scope));
  const { pending, error, setError, run } = useAction();

  const canSubmit = name.trim().length > 0 && scopes.length > 0 && !pending;

  function toggle(scope: ApiKeyScope, on: boolean) {
    setScopes((held) => (on ? [...held, scope] : held.filter((s) => s !== scope)));
    setError(null);
  }

  function submit() {
    run(
      () => mintTokenAction(projectId, { name, description, ttl, scopes }),
      // In the transition, with the list the action drew again: after the
      // `await` it would be an update of its own, and the panel would shut a
      // frame before the new key is listed.
      (r) => startTransition(() => onMinted({ ...r, ingest: scopes.includes(ApiKeyScope.Ingest) })),
    );
  }

  return (
    <SidePanel
      panel={panel}
      titleId={titleId}
      onClose={onClose}
      full
      initialFocus={field}
      returnFocus={returnFocus}
    >
      <form
        className="flex min-h-0 flex-1 flex-col"
        onSubmit={(e) => {
          e.preventDefault();
          if (canSubmit) submit();
        }}
      >
        <SidePanelHeader icon={KeyRound} titleId={titleId} title="New API key" onClose={onClose}>
          <p className="text-sm text-muted-foreground">
            A <code>tm_</code> key for this project: it sends spans, reads the API, or both.
            The plaintext is shown exactly once.
          </p>
        </SidePanelHeader>
        <div className="min-h-0 flex-1 overflow-y-auto px-gutter">
          <SidePanelRow label="Name" htmlFor="key-name">
            <Input
              ref={field}
              id="key-name"
              value={name}
              onChange={(e) => {
                setName(e.target.value);
                setError(null);
              }}
              maxLength={typedBound(MAX_API_KEY_NAME)}
              placeholder="e.g. ci-deploy"
              autoComplete="off"
              required
              disabled={pending}
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? "key-error" : undefined}
              className="max-w-md"
            />
          </SidePanelRow>
          <SidePanelRow label="Description" htmlFor="key-description">
            <Input
              id="key-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              maxLength={typedBound(MAX_API_KEY_DESCRIPTION)}
              placeholder="What is this key for?"
              autoComplete="off"
              disabled={pending}
              className="max-w-md"
            />
            <p className="text-muted-foreground">Optional.</p>
          </SidePanelRow>
          <SidePanelRow label="What it opens">
            <div className="grid max-w-md gap-gutter">
              {API_KEY_SCOPES.map(({ scope, label, detail }) => (
                <div key={scope} className="flex items-center justify-between gap-gutter">
                  <Label htmlFor={`key-scope-${scope}`} className="grid gap-0.5 font-normal">
                    <span>{label}</span>
                    <span className="text-xs text-muted-foreground">{detail}</span>
                  </Label>
                  <Switch
                    id={`key-scope-${scope}`}
                    checked={scopes.includes(scope)}
                    onCheckedChange={(on) => toggle(scope, on)}
                    disabled={pending}
                  />
                </div>
              ))}
            </div>
          </SidePanelRow>
          <SidePanelRow label="Expires" htmlFor="key-ttl">
            <Select value={ttl} onValueChange={setTtl} disabled={pending}>
              <SelectTrigger id="key-ttl" className="w-full max-w-md">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="30d">30 days</SelectItem>
                <SelectItem value="90d">90 days</SelectItem>
                <SelectItem value="1y">1 year</SelectItem>
                <SelectItem value="never">Never (not recommended)</SelectItem>
              </SelectContent>
            </Select>
          </SidePanelRow>
          {error && (
            <p id="key-error" className="py-2 text-sm text-destructive" role="alert">
              {error}
            </p>
          )}
        </div>
        <SidePanelFooter>
          <Button type="submit" disabled={!canSubmit}>
            {pending ? "Minting…" : "Mint key"}
          </Button>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
        </SidePanelFooter>
      </form>
    </SidePanel>
  );
}
