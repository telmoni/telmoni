import { Check } from "lucide-react";
import type { ReactNode } from "react";
import Link from "next/link";

import { StepNumber } from "@/components/ui/step-number";
import { StatusIndicator } from "@/components/status-badge";
import { CopyBox } from "@/components/plaintext-token-dialog";
import { Button } from "@/components/ui/button";
import { AGENT_PROMPT } from "@/lib/first-run";

// A page's first run: what it shows once something lands, and the steps to get
// there, drawn by the server in place of an empty list, so nothing flashes first.

/** The page's first run, its steps numbered under its title. */
export function FirstRun({
  title,
  lead,
  status,
  media,
  children,
}: {
  title: string;
  lead: ReactNode;
  /** A live line above the title, such as the page listening for its first run. */
  status?: ReactNode;
  /** A picture or a video of the page once it has something, beside the steps. */
  media?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section aria-labelledby="first-run" className="mx-auto grid w-full max-w-3xl gap-[calc(3*var(--spacing-gutter))] py-[calc(2*var(--spacing-gutter))] sm:py-[calc(3*var(--spacing-gutter))]">
      <div className="grid justify-items-center gap-gutter text-center">
        {status}
        <h2 id="first-run" className="text-2xl font-semibold tracking-tight">
          {title}
        </h2>
        <p className="max-w-xl text-sm leading-relaxed text-muted-foreground">{lead}</p>
      </div>
      <ol>{children}</ol>
      {media}
    </section>
  );
}

/** One step: its number, or a tick once done, its title, a line on it, and what it offers. */
export function FirstRunStep({
  n,
  title,
  note,
  done = false,
  children,
}: {
  n: number;
  title: string;
  note: ReactNode;
  done?: boolean;
  children?: ReactNode;
}) {
  return (
    <li className="relative grid grid-cols-[auto_minmax(0,1fr)] gap-gutter pb-[calc(2*var(--spacing-gutter))] before:absolute before:top-control before:bottom-0 before:left-[calc(var(--spacing-control)/2)] before:w-px before:bg-border last:pb-0 last:before:hidden">
      <StepNumber aria-hidden="true" className="relative z-10 rounded-full bg-muted text-foreground">
        {done ? <Check className="size-icon text-brand-positive" /> : n}
      </StepNumber>
      <div className="grid min-w-0 gap-gutter">
        <div>
          <h3 className="text-base font-medium">
            {title}
            {done && <span className="sr-only"> (done)</span>}
          </h3>
          <p className="text-sm leading-relaxed text-muted-foreground">{note}</p>
        </div>
        {children}
      </div>
    </li>
  );
}

/** A live line: the page is waiting for something to land, and redraws itself when it does. */
export function Listening({ children }: { children: ReactNode }) {
  return (
    <span role="status" className="flex w-fit items-center gap-gutter text-sm text-muted-foreground">
      <StatusIndicator live className="bg-brand-positive" />
      {children}
    </span>
  );
}

/** The shared setup before a project's traces, agents, sessions or users arrive. */
export function TraceFirstRun({ base, canMint, kind }: {
  base: string;
  canMint: boolean;
  kind: "traces" | "agents" | "sessions" | "users" | "spend";
}) {
  const sessions = kind === "sessions";
  const users = kind === "users";
  return (
    <FirstRun
      status={<Listening>{sessions ? "Listening for traces with a session ID" : users ? "Listening for traces with a user ID" : kind === "agents" ? "Listening for this project’s first agent" : "Listening for this project’s first trace"}</Listening>}
      title={sessions ? "Send your first session" : users ? "See your first user" : kind === "agents" ? "Connect your first agent" : kind === "spend" ? "See what your agents spend" : "Send your first trace"}
      lead="Connect once. This page updates automatically as your data arrives."
    >
      <FirstRunStep
        n={1}
        title="Mint an API key"
        note={canMint ? "Give your application a key with Send spans permission." : "The project's owners and admins mint its keys; ask one for a key with Send spans."}
      >
        {canMint && (
          <div className="flex flex-wrap gap-gutter">
            <Button asChild variant="filled" size="sm"><Link href={`${base}/api-keys`}>Mint a key</Link></Button>
          </div>
        )}
      </FirstRunStep>
      <FirstRunStep
        n={2}
        title="Add tracing to your application"
        note="Give this prompt to the coding agent working in your application."
      >
        <CopyBox value={AGENT_PROMPT} label="prompt" labelVisible />
        <p className="text-sm text-muted-foreground">
          <Link href="/docs/getting-started/connect-an-agent" className="underline underline-offset-4 hover:text-foreground">Set up tracing manually</Link>
        </p>
      </FirstRunStep>
      <FirstRunStep
        n={3}
        title="Run your agent"
        note={sessions ? <>
          Set <code>session_id</code> in the SDK, or <code>gen_ai.conversation.id</code> on the span.
          Coding-agent plugins name sessions automatically.
        </> : users ? <>
          Set <code>user_id</code> in the SDK, or <code>user.id</code> on the span, to your own ID for the
          person the trace is for. It is stored hashed.
        </> : kind === "agents" ? "Send a trace with your agent’s name. Its activity and cost will appear here." : kind === "spend" ? "Trace a model call with its model name and token usage. Spend and token charts appear as data arrives." : "Your first trace appears here within a minute of ending."}
      />
    </FirstRun>
  );
}
