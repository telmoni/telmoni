import { Minus, Play, Plus } from "lucide-react";
import Link from "next/link";

import { InstallTabs, type InstallTab } from "@/components/install-tabs";
import { JsonLd } from "@/components/json-ld";
import {
  BODY,
  LEAD,
  PaperGo,
  PaperHero,
  PaperLink,
  PaperPoint,
  PaperSection,
} from "@/components/paper-page";
import { PaperShell } from "@/components/paper-shell";
import { Button } from "@/components/ui/button";
import { getServerSession } from "@/lib/server/session";
import { DOCS_URL, SKILLS_URL } from "@/lib/site";

import { ForgetPersonalStorage } from "./_forget-personal-storage";

// No `metadata` here on purpose. `layout.tsx` templates a page title as
// `%s · Telmoni`, so a string set on the home route renders the product name
// twice; its `default` is already PRODUCT_NAME, which is what this route wants.

const docs = (path: string) => `${DOCS_URL}${path}`;

// What the product is, over the sentence the page makes — in the words its
// roadmap uses for its three goals — and the paragraph under it: the product
// as it is being built.
const KICKER = "Privacy-first telemetry and monitoring for AI agents";
const CLAIM = "Know when your agents stall, loop or overspend, without sending us a single prompt.";
const PITCH =
  "Every run and the steps inside it, every model and tool call with its tokens, latency and cost, and the alert when something goes wrong. Prompts and completions stay out unless an organization owner turns them on for a project.";

// One way in per tab, each line the book's.
const INSTALL: readonly InstallTab[] = [
  { label: "pip", command: "pip install telmoni" },
  { label: "npm", command: "npm install telmoni" },
  { label: "cargo", command: "cargo add telmoni" },
  { label: "go", command: "go get github.com/telmoni/telmoni/sdk/go" },
  {
    label: "cli",
    command: "curl -fsSL https://raw.githubusercontent.com/telmoni/telmoni-cli/main/install.sh | sh",
  },
  ...(SKILLS_URL
    ? [
        {
          label: "skill",
          command:
            "claude plugin marketplace add telmoni/skills && claude plugin install telmoni@telmoni",
        },
      ]
    : []),
];

type Pointer = { label: string; href: string };

// What the product is: a sentence, what runs today, and, last, what is next.
// Nothing is named here as running before it is built.
const WHAT = {
  title: "What is Telmoni?",
  lead: "An open source, privacy-first telemetry and monitoring platform for AI agents: SDKs in four languages record an agent's runs, ingest keeps them as metadata, and a console reads them as traces, sessions and users.",
  points: [
    {
      title: "Traces",
      body: "Every run and the steps inside it, each model and tool call with its tokens, latency and failures, over OpenTelemetry's own protocol.",
    },
    {
      title: "Sessions and users",
      body: "Runs grouped by conversation and by the end user they served, under ids hashed with your project's own key.",
    },
    {
      title: "Notifications",
      body: "What happens in a project, in the console's feed, on Slack and through signed webhooks.",
    },
    {
      title: "The console agent",
      body: "Ask about a project without leaving the console, each answer citing the pages it read; read-only, on a model you choose.",
    },
    {
      title: "Coding agents",
      body: "A plugin each for Claude Code, Codex, Cursor and OpenCode records their sessions, and the skill teaches any agent to drive the CLI and the API.",
    },
    {
      title: "Self-host",
      body: "One Rust binary and one console, Apache-2.0, on Docker Compose or Kubernetes; you bring the identity provider, the mail transport and the model.",
    },
  ],
  next: "Next, built in the open: alerts over a window with a warning and an alert threshold, to Slack, webhooks and PagerDuty; what each call cost; and content an owner chooses to keep.",
  link: { label: "Read the docs", href: DOCS_URL },
} as const;

const PRIVACY = {
  title: "Built for privacy first",
  points: [
    "Prompts, completions and tool payloads come off a span before it leaves your process, and ingest drops any that arrive: what is kept is metadata — timings, statuses, counts, and model and tool names.",
    "End users' ids are hashed under each project's own key, every field is documented in the field inventory, and every change to your organization is in its audit log.",
  ],
  link: { label: "Read the field inventory", href: docs("/api/field-inventory") },
} as const;

type Door = { title: string; body: string; link: Pointer };

// The console agent in the app, and beside it the three doors a coding
// agent, a terminal or a script comes in by. Each line is the book's.
const DEVELOPERS: { title: string; doors: Door[] } = {
  title: "Work from a terminal, or send an agent",
  doors: [
    {
      title: "The console agent",
      body: "Ask what a project has been doing, who is in it and what changed, each answer with its citations; ⌘J in the console.",
      link: { label: "Read the documentation", href: docs("/self-host/agent") },
    },
    ...(SKILLS_URL
      ? [
          {
            title: "The agent skill",
            body: "Teaches Claude Code, Codex, Cursor and any agent that reads SKILL.md to drive the CLI, call the API and self-host.",
            link: { label: "Install the skill", href: SKILLS_URL },
          },
        ]
      : []),
    {
      title: "The CLI",
      body: "Device sign-in for a terminal and a token for CI; organizations, status and configuration from a shell.",
      link: { label: "Install the CLI", href: docs("/api/cli") },
    },
    {
      title: "The HTTP API",
      body: "The read-only API under /v1, with a project API key, and signed webhooks for what happens.",
      link: { label: "Read the reference", href: docs("/api/reference") },
    },
  ],
};

const FAQ = [
  {
    question: "What runs today?",
    answer:
      "The foundation — organizations and roles, API keys, the audit log, notifications, the console agent and the CLI — and the first of the telemetry: the SDKs record an agent's runs, ingest keeps them as metadata, and the console reads them as traces, sessions and users. Next: alert rules and PagerDuty, what each call cost, and content an owner chooses to keep.",
  },
  {
    question: "What does it store?",
    answer:
      "Metadata: timings, statuses, counts, and model and tool names. Prompts, completions and tool payloads are not sent by the SDKs and are dropped at the door. A content mode that keeps them, for an organization owner to turn on per project, is planned; today the only mode is off.",
  },
  {
    question: "Can I self-host it?",
    answer:
      "Yes, the whole platform: Docker Compose on one machine, or the Helm chart on Kubernetes. You bring the identity provider, the mail transport and the model.",
  },
  {
    question: "How do I get started?",
    answer:
      "Sign up, create a project in the organization your first sign-in makes, make an API key and install an SDK; or install the agent skill and let a coding agent do it.",
  },
];

// The way in: the console for a member, sign-up for a visitor, and the docs
// for both.
function WayIn({ signedIn }: { signedIn: boolean }) {
  return (
    <div className="flex flex-wrap gap-3">
      {signedIn ? (
        <Button asChild size="sm">
          <Link href="/console">Console</Link>
        </Button>
      ) : (
        <Button asChild size="sm">
          {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
          <a href="/auth/signup">Sign up</a>
        </Button>
      )}
      <Button asChild variant="outline" size="sm">
        <PaperLink href={DOCS_URL}>Documentation</PaperLink>
      </Button>
    </div>
  );
}

export default async function SplashPage() {
  const session = await getServerSession();
  const signedIn = session !== null;

  return (
    <PaperShell signedIn={signedIn} framed>
      {/* The structured data describes this page, the one a crawler may index
          (robots.ts), and it names the deployment's origin, which is read per
          request: in the root layout it would also reach the 404 page, which
          is built once with no origin to read. */}
      <JsonLd />
      {signedIn ? null : <ForgetPersonalStorage />}
      <main className="flex flex-1 flex-col">
        {/* ⚠ **The sentence IS the `<h1>`.** The copy is the page's own claim,
            which is what a heading is for; `PRODUCT_NAME` is in the document
            title already and does not want saying twice. */}
        <PaperHero eyebrow={KICKER} title={CLAIM}>
          <p className={LEAD}>{PITCH}</p>
          <InstallTabs tabs={INSTALL} />
          <WayIn signedIn={signedIn} />
        </PaperHero>

        {/* The product shot — a recording of the console — is still to be
            taken; its frame holds the place and says so. */}
        <div className="dots flex aspect-[16/9] w-full items-center justify-center border-b border-border">
          <p className="flex items-center gap-3 border border-dashed border-border bg-background px-5 py-4 text-sm text-muted-foreground">
            <Play aria-hidden="true" className="size-4" />
            A recording of the console, coming soon
          </p>
        </div>

        <PaperSection number="01" title={WHAT.title}>
          <p className={`max-w-[80ch] ${BODY}`}>{WHAT.lead}</p>
          <ul className="flex flex-col gap-4">
            {WHAT.points.map((point) => (
              <PaperPoint key={point.title} title={point.title}>
                {point.body}
              </PaperPoint>
            ))}
          </ul>
          <p className={`max-w-[80ch] ${BODY}`}>{WHAT.next}</p>
          <PaperGo href={WHAT.link.href}>{WHAT.link.label}</PaperGo>
        </PaperSection>

        <PaperSection number="02" title={PRIVACY.title}>
          <ul className="flex flex-col gap-4">
            {PRIVACY.points.map((point) => (
              <PaperPoint key={point}>{point}</PaperPoint>
            ))}
          </ul>
          <PaperGo href={PRIVACY.link.href}>{PRIVACY.link.label}</PaperGo>
        </PaperSection>

        <PaperSection number="03" title={DEVELOPERS.title}>
          <ul className="flex flex-col gap-4">
            {DEVELOPERS.doors.map((door) => (
              <PaperPoint key={door.title} title={door.title}>
                {door.body}{" "}
                <PaperLink
                  href={door.link.href}
                  className="text-foreground underline underline-offset-4 hover:no-underline"
                >
                  {door.link.label}
                </PaperLink>
              </PaperPoint>
            ))}
          </ul>
        </PaperSection>

        <PaperSection number="04" title="Questions and answers">
          <div className="flex flex-col">
            {FAQ.map((entry) => (
              <details key={entry.question} className="group">
                <summary
                  className={`flex cursor-pointer list-none items-start gap-4 py-3 text-foreground [&::-webkit-details-marker]:hidden ${BODY}`}
                >
                  <Plus
                    aria-hidden="true"
                    className="mt-[0.35em] size-3.5 shrink-0 text-muted-foreground/60 group-open:hidden"
                  />
                  <Minus
                    aria-hidden="true"
                    className="mt-[0.35em] hidden size-3.5 shrink-0 text-muted-foreground/60 group-open:block"
                  />
                  <span className="text-foreground">{entry.question}</span>
                </summary>
                <p className={`max-w-[80ch] pb-4 pl-[30px] ${BODY}`}>{entry.answer}</p>
              </details>
            ))}
          </div>
        </PaperSection>
      </main>
    </PaperShell>
  );
}
