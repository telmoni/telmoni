import { ArrowUpRight, ChevronDown, ChevronRight, Minus, Plus } from "lucide-react";

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
import { getServerSession } from "@/lib/server/session";
import { cn } from "@/lib/utils";
import { DOCS_URL, SKILLS_URL } from "@/lib/site";

import { ForgetPersonalStorage } from "./_forget-personal-storage";
import { PromptRow } from "./_prompt-row";
import { WayIn } from "./_way-in";

// No `metadata` here on purpose. `layout.tsx` templates a page title as
// `%s · Telmoni`, so a string set on the home route renders the product name
// twice; its `default` is already PRODUCT_NAME, which is what this route wants.

const docs = (path: string) => `${DOCS_URL}${path}`;

// What the product is, over the sentence the page makes, and the paragraph
// under it: the product as it is being built.
const KICKER = "Privacy-first observability for AI agents";
// Beside the kicker, so nobody takes the product for finished.
const STAGE = "Beta";
const CLAIM = "Know when your agents stall, loop or overspend, and keep their prompts only where you choose to.";
const PITCH =
  "Every run and the steps inside it, every model and tool call with its tokens, latency and cost, scored, and the alert when something goes wrong. Prompts and completions stay in your process until a project's owner and your code both turn content on.";

// One way in per tab, each line the book's.
const INSTALL: readonly InstallTab[] = [
  { label: "pip", command: "pip install telmoni" },
  { label: "npm", command: "npm install telmoni" },
  { label: "cargo", command: "cargo add telmoni" },
  { label: "go", command: "go get github.com/telmoni/telmoni/sdk/go" },
  {
    label: "cli",
    command: "curl -fsSL https://telmoni.com/install.sh | sh",
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

// What Telmoni is, as a run reaches it, drawn left to right: what sends it,
// the one way in, and what the console does with it, each a page of the
// book. Nothing is named here as running before it is built.
const FLOW = {
  title: "What is Telmoni?",
  lead: "An open source, privacy-first observability platform for AI agents. SDKs in four languages, a framework's own instrumentation or any OpenTelemetry exporter send an agent's runs; the console traces them, scores them and alerts on them.",
  sources: [
    { title: "Your agents", body: "Python, TypeScript, Go and Rust", href: docs("/api/sdks") },
    { title: "Frameworks", body: "OpenAI, Anthropic, LangChain, CrewAI, LiteLLM, Vercel AI SDK", href: docs("/api/frameworks") },
    { title: "Coding agents", body: "Claude Code, Codex, Cursor, OpenCode", href: docs("/api/coding-agents") },
  ],
  way: { title: "SDKs or OpenTelemetry", body: "OTLP over HTTP, behind a project's API key", href: docs("/api/ingest") },
  groups: [
    {
      name: "Observe",
      says: "Runs, steps and calls with tokens, latency and cost; content where kept.",
      pages: [
        { label: "Traces", href: docs("/monitoring/traces") },
        { label: "Sessions", href: docs("/monitoring/sessions") },
        { label: "Users", href: docs("/monitoring/users") },
        { label: "Dashboards", href: docs("/monitoring/dashboards") },
      ],
    },
    {
      name: "Evaluate",
      says: "Scores from your code and team; evaluators on your own model key.",
      pages: [
        { label: "Scores", href: docs("/monitoring/scores") },
        { label: "Evaluators", href: docs("/workspace/evaluators") },
      ],
    },
    {
      name: "Alert",
      says: "Thresholds from your agents' history; objectives on an error budget.",
      pages: [
        { label: "Alerts", href: docs("/monitoring/alerts") },
        { label: "Objectives", href: docs("/monitoring/objectives") },
      ],
    },
  ],
  through: [
    { label: "Agent", href: docs("/self-host/agent") },
    { label: "CLI", href: docs("/api/cli") },
    { label: "API", href: docs("/api/reference") },
    { label: "Webhooks", href: docs("/integrations/webhooks") },
  ],
} as const;

const PRIVACY = {
  title: "Built for privacy first",
  points: [
    "Content leaves your process only when two locks are both on: the switch in your SDK and the project's content mode, which only the organization's owner turns on. With either off, a span is sent and kept as metadata alone — timings, statuses, counts, and model and tool names.",
    "End users' ids are hashed under each project's own key, every field is documented in the field inventory, and every change to your organization is in its audit log.",
  ],
  link: { label: "Read the field inventory", href: docs("/api/field-inventory") },
} as const;

// What the SDKs and the exporter already speak to, each a chip into its page.
const STACK: { title: string; chips: Pointer[] }[] = [
  {
    title: "Languages",
    chips: [
      { label: "Python", href: docs("/api/sdks") },
      { label: "TypeScript", href: docs("/api/sdks") },
      { label: "Go", href: docs("/api/sdks") },
      { label: "Rust", href: docs("/api/sdks") },
      { label: "Any OpenTelemetry exporter", href: docs("/api/ingest") },
    ],
  },
  {
    title: "Frameworks and model SDKs",
    chips: [
      { label: "OpenAI SDK", href: docs("/api/frameworks#openai-sdk") },
      { label: "Anthropic SDK", href: docs("/api/frameworks#anthropic-sdk") },
      { label: "LangChain and LangGraph", href: docs("/api/frameworks#langchain-and-langgraph") },
      { label: "OpenAI Agents SDK", href: docs("/api/frameworks#openai-agents-sdk") },
      { label: "CrewAI", href: docs("/api/frameworks#crewai") },
      { label: "LiteLLM", href: docs("/api/frameworks#litellm") },
      { label: "Vercel AI SDK", href: docs("/api/frameworks#vercel-ai-sdk") },
    ],
  },
  {
    title: "Coding agents",
    chips: [
      { label: "Claude Code", href: docs("/api/coding-agents#claude-code") },
      { label: "Codex", href: docs("/api/coding-agents#codex") },
      { label: "Cursor", href: docs("/api/coding-agents#cursor") },
      { label: "OpenCode", href: docs("/api/coding-agents#opencode") },
    ],
  },
  {
    title: "Where incidents go",
    chips: [
      { label: "Slack", href: docs("/integrations/slack") },
      { label: "Webhooks", href: docs("/integrations/webhooks") },
      { label: "The console's feed", href: docs("/integrations/notifications") },
    ],
  },
];

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

// Why it is built the way it is, a line each.
const WHY: { label: string; body: string }[] = [
  { label: "Open source", body: "Apache-2.0, the whole platform: read the code, self-host it, change it." },
  { label: "OpenTelemetry native", body: "Standard spans over OTLP, so an exporter you already run sends to it." },
  { label: "One binary", body: "One Rust server and one console, on Docker Compose or Kubernetes." },
  { label: "Your own model keys", body: "Evaluators judge on the keys a project connects, never ours." },
  { label: "Made for agents", body: "A skill, a CLI and an API a coding agent can drive, and plugins that trace the agents themselves." },
];

// What to ask a coding agent, once the skill is installed; each copied whole.
const PROMPTS: { label: string; prompt: string }[] = [
  {
    label: "Tracing",
    prompt: "Use the Telmoni skill to add tracing to this application with the Telmoni SDK, following its best practices.",
  },
  {
    label: "Alerts",
    prompt: "Use the Telmoni skill to set up alerts for this project's agents: failures, p95 latency and a monthly budget.",
  },
  {
    label: "Scores",
    prompt: "Use the Telmoni skill to send scores from this application's own checks, and suggest an evaluator for its answers.",
  },
];

const FAQ = [
  {
    question: "What runs today?",
    answer:
      "The foundation — organizations and roles, API keys, the audit log, notifications, the console agent and the CLI — and the telemetry on it: the SDKs record an agent's runs and what each call cost, and stop a run at the caps you set; the console reads them as traces, sessions, users and dashboards, keeps their content where a project turns it on, scores them and judges them with evaluators on your own model keys, and alerts on them and on objectives, to Slack and webhooks.",
  },
  {
    question: "What does it store?",
    answer:
      "Metadata on every project: timings, statuses, counts, and model and tool names. Content — prompts, completions, tool payloads, images and files — only on a project whose owner turned its content mode on, from code whose SDK sends it; anything else is dropped at the door.",
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
        <PaperHero badge={STAGE} eyebrow={KICKER} title={CLAIM}>
          <p className={LEAD}>{PITCH}</p>
          <WayIn signedIn={signedIn} docsHref={DOCS_URL} />
          <div className="w-full max-w-2xl">
            <InstallTabs tabs={INSTALL} />
          </div>
        </PaperHero>

        <PaperSection title={FLOW.title}>
          <p className={`max-w-[80ch] ${BODY}`}>{FLOW.lead}</p>
          <Flow />
          <PaperGo href={DOCS_URL}>Read the docs</PaperGo>
        </PaperSection>

        <PaperSection title={PRIVACY.title}>
          <ul className="flex max-w-[80ch] flex-col gap-4">
            {PRIVACY.points.map((point) => (
              <PaperPoint key={point}>{point}</PaperPoint>
            ))}
          </ul>
          <PaperGo href={PRIVACY.link.href}>{PRIVACY.link.label}</PaperGo>
        </PaperSection>

        <PaperSection title="Works with your stack">
          <p className={`max-w-[80ch] ${BODY}`}>
            Any language and framework that speaks OpenTelemetry, and these with a page of their own.
          </p>
          <div className="grid gap-gutter sm:grid-cols-2">
            {STACK.map((group) => (
              <div key={group.title} className="flex flex-col gap-gutter border border-border p-4">
                <h3 className="text-sm font-medium text-foreground">{group.title}</h3>
                <ul className="flex flex-wrap gap-2">
                  {group.chips.map((chip) => (
                    <li key={chip.label}>
                      <Chip href={chip.href}>{chip.label}</Chip>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        </PaperSection>

        <PaperSection title={DEVELOPERS.title}>
          <ul className="flex max-w-[80ch] flex-col gap-4">
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

        <PaperSection title="Why Telmoni?">
          <dl className="flex max-w-4xl flex-col">
            {WHY.map((row) => (
              <div
                key={row.label}
                className="grid gap-1 border-b border-border py-4 first:pt-0 last:border-b-0 last:pb-0 sm:grid-cols-[1fr_3fr] sm:gap-8"
              >
                <dt className="text-[15px] font-medium text-foreground">{row.label}</dt>
                <dd className={BODY}>{row.body}</dd>
              </div>
            ))}
          </dl>
        </PaperSection>

        {SKILLS_URL && (
          <PaperSection title="Start in five minutes">
            <p className={`max-w-[80ch] ${BODY}`}>
              Install the agent skill in Claude Code, Codex, Cursor or any agent that reads SKILL.md,
              then ask it for what you want.
            </p>
            <ul className="flex max-w-4xl flex-col divide-y divide-border border border-border">
              {PROMPTS.map((entry) => (
                <li key={entry.label}>
                  <PromptRow label={entry.label} prompt={entry.prompt} />
                </li>
              ))}
            </ul>
            <PaperGo href={SKILLS_URL}>Get the skill</PaperGo>
          </PaperSection>
        )}

        <PaperSection title="Questions and answers">
          <div className="flex flex-col">
            {FAQ.map((entry) => (
              <details key={entry.question} className="group">
                <summary
                  className={`flex cursor-pointer list-none items-start gap-4 py-gutter text-foreground [&::-webkit-details-marker]:hidden ${BODY}`}
                >
                  <Plus
                    aria-hidden="true"
                    className="mt-[0.35em] size-3.5 shrink-0 text-muted-foreground group-open:hidden"
                  />
                  <Minus
                    aria-hidden="true"
                    className="mt-[0.35em] hidden size-3.5 shrink-0 text-muted-foreground group-open:block"
                  />
                  <span className="text-foreground">{entry.question}</span>
                </summary>
                <p className={`max-w-[80ch] pb-4 pl-7.5 ${BODY}`}>{entry.answer}</p>
              </details>
            ))}
          </div>
        </PaperSection>
      </main>
    </PaperShell>
  );
}

/** A label inside a box of the flow: the eyebrow's case and size, tracked
 *  as a word rather than as a page's opening line. */
const CARD_LABEL = "text-[11px] font-medium uppercase tracking-wide text-muted-foreground";

/** A link as a small outlined chip, as the stack and the flow list them:
 *  a thumb's height on a phone, a line's on a wider page. */
function Chip({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <PaperLink
      href={href}
      className="inline-flex min-h-9 items-center border border-border px-2.5 text-[13px] leading-none text-muted-foreground transition-colors hover:border-foreground/40 hover:text-foreground sm:min-h-7"
    >
      {children}
    </PaperLink>
  );
}

/** A box of the flow that is a page of the book: its title, what it says,
 *  and the arrow every link out of the flow carries. */
function FlowLink({ href, title, body }: { href: string; title: string; body: string }) {
  return (
    <PaperLink
      href={href}
      className="group flex min-h-11 items-start justify-between gap-gutter py-gutter first:pt-0 last:pb-0"
    >
      <span className="flex flex-col gap-1">
        <span className="text-[15px] font-medium text-foreground group-hover:underline group-hover:underline-offset-4">
          {title}
        </span>
        <span className="text-[13px] text-muted-foreground">{body}</span>
      </span>
      <ArrowUpRight aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-muted-foreground group-hover:text-foreground" />
    </PaperLink>
  );
}

/** The line from one column of the flow to the next, with its head: across
 *  on a wide page, down on a narrow one. */
function Arrow() {
  return (
    <div aria-hidden="true" className="flex items-center justify-center text-muted-foreground/70">
      <div className="flex h-8 flex-col items-center md:hidden">
        <span className="w-px flex-1 bg-current" />
        <ChevronDown className="-mt-1.5 size-3.5" />
      </div>
      <div className="hidden w-full items-center md:flex">
        <span className="h-px flex-1 bg-current" />
        <ChevronRight className="-ml-1.5 size-3.5" />
      </div>
    </div>
  );
}

/** How a run reaches the console and what it does there, as boxes left to right. */
function Flow() {
  const box = "flex flex-col border border-border bg-background p-4";
  return (
    <figure aria-label="How a run reaches Telmoni" className="flex flex-col">
      <div className="grid md:grid-cols-[minmax(0,0.9fr)_2.5rem_minmax(0,0.75fr)_2.5rem_minmax(0,1.6fr)] md:items-center">
        <div className={cn(box, "divide-y divide-border")}>
          {FLOW.sources.map((source) => (
            <FlowLink key={source.title} {...source} />
          ))}
        </div>
        <Arrow />
        <div className={box}>
          <FlowLink {...FLOW.way} />
        </div>
        <Arrow />
        <div className={cn(box, "gap-4")}>
          <span className="text-[15px] font-semibold text-foreground">Telmoni</span>
          {FLOW.groups.map((group) => (
            <div key={group.name} className="flex flex-col gap-2">
              <span className={CARD_LABEL}>{group.name}</span>
              <p className="text-[13px] leading-normal text-muted-foreground">{group.says}</p>
              <ul className="flex flex-wrap gap-1.5">
                {group.pages.map((page) => (
                  <li key={page.label}>
                    <Chip href={page.href}>{page.label}</Chip>
                  </li>
                ))}
              </ul>
            </div>
          ))}
          <div className="flex flex-col gap-2 border-t border-border pt-4">
            <span className={CARD_LABEL}>Every feature through</span>
            <ul className="flex flex-wrap gap-1.5">
              {FLOW.through.map((page) => (
                <li key={page.label}>
                  <Chip href={page.href}>{page.label}</Chip>
                </li>
              ))}
            </ul>
          </div>
        </div>
      </div>
      <figcaption className="sr-only">
        Your agents, frameworks and coding agents send their runs through the SDKs
        or OpenTelemetry to Telmoni, which observes them as traces, sessions, users and dashboards, evaluates
        them with scores and evaluators, and alerts on them and on objectives; every feature is also reached
        through the agent, the CLI, the API and webhooks.
      </figcaption>
    </figure>
  );
}
