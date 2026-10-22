import { Minus, Plus } from "lucide-react";
import Link from "next/link";

import { JsonLd } from "@/components/json-ld";
import { Mark } from "@/components/mark";
import { PaperShell } from "@/components/paper-shell";
import { SplashHero } from "@/components/splash-hero";
import { Button } from "@/components/ui/button";
import { getServerSession } from "@/lib/server/session";
import { DOCS_URL, SECURITY_POLICY_URL, SKILLS_URL } from "@/lib/site";
import { cn } from "@/lib/utils";

import { ForgetPersonalStorage } from "./_forget-personal-storage";

// No `metadata` here on purpose. `layout.tsx` templates a page title as
// `%s · Telmoni`, so a string set on the home route renders the product name
// twice; its `default` is already PRODUCT_NAME, which is what this route wants.

const docs = (path: string) => `${DOCS_URL}${path}`;

// The line above the claim: what this is, and where it stands.
const CAPTIONS = ["Open source · Apache-2.0", "Built in the open"];

// The sentence the page makes, the phrase it leans on, and the paragraph
// under it: the product as it is being built, in the words its roadmap uses
// for its three goals.
const CLAIM = "Know when your agents stall, loop or overspend, without sending us a single prompt.";
const HIGHLIGHT = "stall, loop or overspend";
const PITCH =
  "Every run and the steps inside it, every model and tool call with its tokens, latency and cost, the bounds each agent keeps, and the alert when one breaks them. Prompts and completions stay out unless an organization owner turns them on for a project.";

// True today; it moves as the roadmap does, and says what is not built yet.
const STATUS =
  "What runs today is the foundation — organizations and roles, API keys, the audit log, notifications and the console agent — and the first of the telemetry on it: SDKs in four languages that record an agent's runs, ingest that keeps them as metadata, and a console that reads them as traces, sessions and users. Monitors and alerts are next, built in the open.";

// What the product is for, one cell per goal, in the order they are built:
// a small motion that says it, a line, then three points.
const GOALS = [
  {
    title: "Telemetry for agents",
    glyph: "spans",
    lead: "Every run and the steps inside it, from any framework.",
    points: [
      "Runs, steps, model and tool calls over OpenTelemetry's own protocol, or a curl from a crontab",
      "Tokens, latency, cost and failures, per run, per project, per end user",
      "Spans kept as long as retention says; hourly totals a year back",
    ],
  },
  {
    title: "Monitoring",
    glyph: "pulse",
    lead: "Every agent held to its bounds, and heard when it breaks them.",
    points: [
      "A schedule, a stall timeout, a step cap, a cost cap",
      "Alerts with routing, severity, deduplication and maintenance windows",
      "Slack, webhooks and PagerDuty",
    ],
  },
  {
    title: "Privacy first",
    glyph: "redact",
    lead: "Metadata by default: timings, statuses, counts, names, costs.",
    points: [
      "Prompts and completions stay out unless an owner turns them on, per project",
      "Kept as they arrive, or sealed under a key the server does not hold",
      "Every read audited, every deletion receipted, every field documented",
    ],
  },
] as const;

type Pointer = { label: string; href: string };

// The open platform, three cells of links into the documentation, each with
// its own motion in the corner as the goals above have.
const OPEN: { title: string; glyph: GlyphKind; blurb: string; links: Pointer[] }[] = [
  {
    title: "Self-host",
    glyph: "hosts",
    blurb: "The whole platform on your own infrastructure. You bring the identity provider, the mail transport and the model.",
    links: [
      { label: "Docker Compose", href: docs("/self-host/docker-compose") },
      { label: "Kubernetes, with Helm", href: docs("/self-host/kubernetes") },
      { label: "Production hardening", href: docs("/self-host/production") },
      { label: "The agent on your own model", href: docs("/self-host/agent") },
    ],
  },
  {
    title: "Security as mechanism",
    glyph: "lock",
    blurb: "Isolation the database enforces, an audit record in the same transaction as the change, and deletion that ends.",
    links: [
      { label: "Tenant isolation", href: `${SECURITY_POLICY_URL}#tenant-isolation` },
      { label: "Secrets", href: `${SECURITY_POLICY_URL}#secrets` },
      { label: "Audit", href: docs("/workspace/audit-log") },
      { label: "Data deletion", href: docs("/workspace/data-deletion") },
    ],
  },
];

// Developers and agents: the console agent in the app, and beside it the
// three doors a coding agent, a terminal or a script comes in by. Each line
// is the documentation's.
const AGENT = {
  tag: "In-app",
  title: "The console agent",
  body: "Ask about a project without leaving the console: what it has been doing, who is in it and what changed, each answer citing the pages it read. Read-only, on a model you choose.",
  asks: [
    { title: "Find what failed", body: "A delivery that failed, and when." },
    { title: "Find who did what", body: "Members, roles and every change, from the audit log." },
    { title: "Find the page", body: "In the docs, where the deployment indexes them." },
  ],
  link: { label: "Read the documentation", href: docs("/self-host/agent") },
} as const;

type Door = { tag: string; title: string; body: string; link: Pointer };

const DOORS: Door[] = [
  ...(SKILLS_URL
    ? [
        {
          tag: "Coding agents",
          title: "The agent skill",
          body: "Teaches Claude Code, Codex, Cursor and any agent that reads SKILL.md to drive the CLI, call the API and self-host.",
          link: { label: "Install the skill", href: SKILLS_URL },
        },
      ]
    : []),
  {
    tag: "Terminal",
    title: "The CLI",
    body: "Device sign-in for a terminal and a token for CI; organizations, status and configuration from a shell.",
    link: { label: "Install the CLI", href: docs("/api/cli") },
  },
  {
    tag: "Scripts",
    title: "The HTTP API",
    body: "The read-only API under /v1, with a project API key, and signed webhooks for what happens.",
    link: { label: "Read the reference", href: docs("/api/reference") },
  },
];

const WHY = [
  {
    title: "Privacy first",
    body: "Metadata by default. What a project keeps of content is its organization owner's switch: off today, with on and sealed to come.",
  },
  {
    title: "Open source",
    body: "Apache-2.0, every feature. Self-host it on a laptop or a cluster, or let somebody run it for you.",
  },
  {
    title: "One binary",
    body: "The server is one Rust binary and the console one Next.js app, with two databases and a cache beside them.",
  },
  {
    title: "OpenTelemetry native",
    body: "Our SDKs today, on OpenTelemetry's own SDK and protocol; any instrumentation that speaks the GenAI conventions, and a curl from a crontab, next.",
  },
  {
    title: "Built in the open",
    body: "One maintainer; the code, the docs and the skills on GitHub; and every change to your organization in its audit log.",
  },
];

const FAQ = [
  {
    question: "What is Telmoni?",
    answer:
      "A privacy-first telemetry and monitoring platform for AI agents: every run and the steps inside it, every model and tool call with its tokens, latency and cost, the bounds each agent keeps, and the alert when one breaks them. It is open source, Apache-2.0, and runs as one binary you can self-host.",
  },
  {
    question: "What runs today?",
    answer:
      "The foundation — organizations and roles, API keys, the audit log, notifications, the console agent and the CLI — and the first of the telemetry: the SDKs record an agent's runs, ingest keeps them as metadata, and the console reads them as traces, sessions and users. Monitors and alerts are next.",
  },
  {
    question: "What does it store?",
    answer:
      "Metadata: timings, statuses, counts, model and tool names, costs. Prompts, completions and tool payloads are not sent by the SDKs and are dropped at the door. A content mode that keeps them, for an organization owner to turn on per project, is planned; today the only mode is off.",
  },
  {
    question: "Can I self-host it?",
    answer:
      "Yes, the whole platform: Docker Compose on one machine, or the Helm chart on Kubernetes. You bring the identity provider, the mail transport and the model.",
  },
  {
    question: "How do I get started?",
    answer:
      "Sign up, create a project in the organization your first sign-in makes, make an API key and install the CLI; or install the agent skill and let a coding agent do it.",
  },
];

// A link into the console — the book among its pages — is routed; one out
// of it opens in a new tab, so the page stays.
function Out({
  href,
  className,
  children,
}: {
  href: string;
  className?: string;
  children: React.ReactNode;
}) {
  if (href.startsWith("/")) {
    return (
      <Link href={href} className={className}>
        {children}
      </Link>
    );
  }
  return (
    <a href={href} target="_blank" rel="noreferrer" className={className}>
      {children}
    </a>
  );
}

// Privacy first, as a document reads once it is through the door: on each
// line one short field kept — a timing, a status, a count — and the words
// beside it, the prompt, under a marker. Widths in pixels of the glyph's 64,
// two apart; the marker covers the words that are not the field, which sit
// together at one end of the line.
const GLYPH_GAP = 2;
const REDACTED_LINES = [
  { words: [12, 22, 9, 15], kept: 0, delay: 0 },
  { words: [18, 10, 14, 16], kept: 3, delay: 300 },
  { words: [9, 20, 13], kept: 0, delay: 600 },
] as const;

function markerOver(words: readonly number[], kept: number) {
  const starts = words.map((_, i) => words.slice(0, i).reduce((at, w) => at + w + GLYPH_GAP, 0));
  const content = words.map((_, i) => i).filter((i) => i !== kept);
  const first = content[0] ?? 0;
  const last = content[content.length - 1] ?? 0;
  return { left: starts[first], width: starts[last] + (words[last] ?? 0) - (starts[first] ?? 0) };
}

// A goal's motion, in a cell's corner: spans arriving one under another, a
// schedule's pulse, and a document's words going under a marker line by line
// while the field kept on each line stays. Decorative, and still under
// reduced motion.
type GlyphKind = "spans" | "pulse" | "redact" | "hosts" | "lock";

function Glyph({ kind }: { kind: GlyphKind }) {
  // Self-host: three machines of your own, their lights coming on in turn.
  if (kind === "hosts") {
    return (
      <span aria-hidden="true" className="flex w-16 flex-col items-start gap-0.5">
        {[0, 1, 2].map((i) => (
          <span
            key={i}
            className="flex h-2 w-11 items-center gap-1 rounded-sm border border-foreground/40 px-1"
          >
            <span className="h-px flex-1 bg-foreground/30" />
            <span
              className="glyph-blink size-1 [border-radius:9999px] bg-foreground/80"
              style={{ animationDelay: `${i * 400}ms` }}
            />
          </span>
        ))}
      </span>
    );
  }
  // Security as mechanism: a lock whose shackle pops open, pivots, returns, and snaps shut.
  if (kind === "lock") {
    return (
      <span aria-hidden="true" className="flex w-16">
        {/* Pulled left by the body's inset, so the lock's edge, not its box's,
            lines up with the title under it. */}
        <svg
          className="-ml-1 size-7 overflow-visible"
          viewBox="0 0 28 28"
          fill="none"
          xmlns="http://www.w3.org/2000/svg"
        >
          {/* Shackle: heel (left) remains seated in the body; toe (right) unlatches and clears the socket */}
          <path
            className="glyph-shackle text-foreground/60"
            d="M 8.5 16 V 7 A 5.5 5.5 0 0 1 19.5 7 V 13"
            stroke="currentColor"
            strokeWidth="2.25"
            strokeLinecap="round"
          />
          {/* Body, laid on a card-coloured one of its own shape: its ink is
              translucent, and the shackle's seated legs showed through it. */}
          <rect
            x="4"
            y="11.5"
            width="20"
            height="14"
            rx="2"
            className="text-card"
            fill="currentColor"
          />
          <rect
            x="4"
            y="11.5"
            width="20"
            height="14"
            rx="2"
            className="text-foreground/70"
            fill="currentColor"
          />
          {/* Keyhole */}
          <circle cx="14" cy="17" r="1.25" className="text-card" fill="currentColor" />
          <rect
            x="13.2"
            y="17"
            width="1.6"
            height="3"
            rx="0.5"
            className="text-card"
            fill="currentColor"
          />
        </svg>
      </span>
    );
  }
  if (kind === "spans") {
    return (
      <span aria-hidden="true" className="flex w-16 flex-col gap-1">
        {[70, 45, 90, 30].map((width, i) => (
          <span
            key={width}
            className="glyph-grow block h-1 rounded-sm bg-foreground/60"
            style={{ width: `${width}%`, marginLeft: `${i * 6}%`, animationDelay: `${i * 220}ms` }}
          />
        ))}
      </span>
    );
  }
  if (kind === "pulse") {
    return (
      <span aria-hidden="true" className="flex h-7 w-16 items-center gap-3">
        {[0, 1, 2].map((i) => (
          <span key={i} className="relative flex size-2">
            <span
              className="glyph-ping absolute inline-flex size-full [border-radius:9999px] bg-foreground/40"
              style={{ animationDelay: `${i * 400}ms` }}
            />
            <span className="relative inline-flex size-2 [border-radius:9999px] bg-foreground/70" />
          </span>
        ))}
      </span>
    );
  }
  return (
    <span aria-hidden="true" className="flex w-16 flex-col gap-1">
      {REDACTED_LINES.map(({ words, kept, delay }, line) => {
        const marker = markerOver(words, kept);
        return (
          <span key={line} className="relative flex h-1 gap-0.5">
            {words.map((width, i) => (
              <span
                key={i}
                className={cn(
                  "block h-1 shrink-0 rounded-sm",
                  i === kept ? "bg-foreground/70" : "bg-foreground/30",
                )}
                style={{ width }}
              />
            ))}
            <span
              className="redact absolute -inset-y-px rounded-sm bg-foreground"
              style={{ left: marker.left, width: marker.width, animationDelay: `${delay}ms` }}
            />
          </span>
        );
      })}
    </span>
  );
}

// The small mono line a product site sets above a heading.
function Label({ className, children }: { className?: string; children: React.ReactNode }) {
  return (
    <p
      className={cn(
        "font-mono text-[11px] uppercase tracking-label text-muted-foreground",
        className,
      )}
    >
      {children}
    </p>
  );
}

function Tag({ children }: { children: React.ReactNode }) {
  return (
    <span className="w-fit rounded-sm border border-border px-1.5 py-0.5 text-[11px] text-muted-foreground">
      {children}
    </span>
  );
}

// A section opens with its label, its heading with one phrase marked, and
// a line under it.
function SectionHead({
  label,
  title,
  blurb,
}: {
  label: string;
  title: React.ReactNode;
  blurb?: string;
}) {
  return (
    <div className="rise-in flex flex-col gap-3">
      <Label>{label}</Label>
      <h2 className="text-[28px] font-semibold leading-[1.15] tracking-tight text-foreground sm:text-[32px]">
        {title}
      </h2>
      {blurb && <p className="max-w-[64ch] text-[15px] leading-[1.5] text-muted-foreground">{blurb}</p>}
    </div>
  );
}

// A hairline grid: the cells share one rule, as a product site draws them.
// ⚠ Each cell draws the rule itself, its top and left borders, pulled a pixel
// up and left so the first row's and column's fall under the frame, which
// clips them. Not the grid's ink through a one-pixel gap: an uneven split — a
// third and two thirds — lands the gap on a fraction of a pixel, which the
// browser paints as two, and the rule reads doubled. Not a ring either: a
// shadow is not snapped to the screen's pixels as a border is, so on a 1.25×
// or 1.5× display it draws heavier than the frame.
function Cells({ className, children }: { className?: string; children: React.ReactNode }) {
  return (
    <div
      className={cn(
        "rise-in grid overflow-hidden rounded-xl border border-border",
        className,
      )}
    >
      {children}
    </div>
  );
}


function Chip({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <Out
      href={href}
      className="inline-flex items-center rounded-sm border border-border px-2 py-1 text-[13px] text-foreground transition-colors hover:bg-muted/60"
    >
      {children}
    </Out>
  );
}

function Arrow({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <Out href={href} className="text-[13px] font-medium text-foreground hover:underline">
      {children} →
    </Out>
  );
}

// The way in: the console for a member, sign-up for a visitor, and the docs
// for both.
function WayIn({ signedIn, align = "center" }: { signedIn: boolean; align?: "start" | "center" }) {
  return (
    <div className={cn("flex flex-wrap gap-3", align === "start" ? "justify-start" : "justify-center")}>
      {signedIn ? (
        <Button asChild>
          <Link href="/console">Console</Link>
        </Button>
      ) : (
        <Button asChild>
          {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
          <a href="/auth/signup">Sign up</a>
        </Button>
      )}
      <Button asChild variant="outline">
        <Out href={DOCS_URL}>Documentation</Out>
      </Button>
    </div>
  );
}

export default async function SplashPage() {
  const session = await getServerSession();
  const signedIn = session !== null;

  return (
    <PaperShell signedIn={signedIn} className="dark">
      {/* The structured data describes this page, the one a crawler may index
          (robots.ts), and it names the deployment's origin, which is read per
          request: in the root layout it would also reach the 404 page, which
          is built once with no origin to read. */}
      <JsonLd />
      {signedIn ? null : <ForgetPersonalStorage />}
      {/* The same container as `/plans`, so the two public pages line up. */}
      <main className="mx-auto flex w-full max-w-6xl flex-1 flex-col gap-16 px-3.5 py-8 sm:gap-20">
        {/* ⚠ **The sentence IS the `<h1>`**, drawn by the hero beside the run
            board. The copy is the page's own claim, which is what a heading is
            for; `PRODUCT_NAME` is in the document title already and does not
            want saying twice. */}
        <SplashHero
          claim={CLAIM}
          highlight={HIGHLIGHT}
          pitch={PITCH}
          status={STATUS}
          captions={CAPTIONS}
        >
          <WayIn signedIn={signedIn} align="start" />
        </SplashHero>

        <section className="flex flex-col gap-6">
          <SectionHead
            label="What it is for"
            title={
              <>
                Telemetry, monitoring, <Mark>privacy</Mark>.
              </>
            }
            blurb="Built in this order on the foundation below, for agents in production: a service, a nightly batch, a coding session, a graph."
          />
          {/* The cells share their rows — motion, title, lead, list — so the
              rule above each list is one line across the row, where the
              longest lead ends, rather than three at three heights. */}
          <Cells className="bg-card">
            <div className="grid grid-cols-1 gap-6 p-5 sm:grid-cols-3 sm:gap-0 sm:grid-rows-[auto_auto_auto_1fr]">
              {GOALS.map((goal, i) => (
                <div
                  key={goal.title}
                  className={cn(
                    "row-span-4 grid grid-rows-subgrid gap-3",
                    i > 0 && "border-t border-border pt-6 sm:border-t-0 sm:border-l sm:border-border sm:pt-0 sm:pl-6",
                    i < GOALS.length - 1 && "sm:pr-6",
                  )}
                >
                  <Glyph kind={goal.glyph} />
                  <h3 className="text-[15px] font-medium text-foreground">{goal.title}</h3>
                  <p className="text-sm leading-[1.5] text-muted-foreground">{goal.lead}</p>
                  <ul className="flex flex-col gap-2 border-t border-border pt-3 text-sm leading-[1.4] text-muted-foreground">
                    {goal.points.map((point) => (
                      <li key={point} className="flex gap-2">
                        <span aria-hidden="true" className="mt-[0.55em] size-1 shrink-0 rounded-full bg-muted-foreground" />
                        {point}
                      </li>
                    ))}
                  </ul>
                </div>
              ))}
            </div>
          </Cells>
        </section>

        <section className="flex flex-col gap-6">
          <SectionHead
            label="Open source"
            title={
              <>
                <Mark>Open platform.</Mark> Open source.
              </>
            }
            blurb="The whole platform is Apache-2.0: the server, the console and the docs it serves, the CLI and the SDKs, and the agent skills. Nothing is held back."
          />
          <Cells className="bg-card">
            <div className="grid grid-cols-1 gap-6 p-5 sm:grid-cols-2 sm:grid-rows-[auto_auto_auto_1fr] sm:gap-0">
              {OPEN.map((box, i) => (
                <div
                  key={box.title}
                  className={cn(
                    "row-span-4 grid grid-rows-subgrid gap-3",
                    i > 0 && "border-t border-border pt-6 sm:border-t-0 sm:border-l sm:border-border sm:pt-0 sm:pl-6",
                    i < OPEN.length - 1 && "sm:pr-6",
                  )}
                >
                  <Glyph kind={box.glyph} />
                  <h3 className="text-[15px] font-medium text-foreground">{box.title}</h3>
                  <p className="text-sm leading-[1.5] text-muted-foreground">{box.blurb}</p>
                  <ul className="flex flex-wrap content-start gap-2 border-t border-border pt-3">
                    {box.links.map((link) => (
                      <li key={link.href}>
                        <Chip href={link.href}>{link.label}</Chip>
                      </li>
                    ))}
                  </ul>
                </div>
              ))}
            </div>
          </Cells>
        </section>

        <section className="flex flex-col gap-6">
          <SectionHead
            label="Developers and agents"
            title={
              <>
                Work from the console or a terminal, or <Mark>send an agent</Mark>.
              </>
            }
            blurb="Work in the console or from a terminal. The console agent answers about your project with citations; the skill, the CLI and the API connect coding agents to Telmoni."
          />
          <Cells className="bg-card">
            <div className="flex flex-col gap-4 p-5 lg:grid lg:grid-cols-[1.2fr_1fr] lg:gap-8">
              <div className="flex flex-col gap-4">
                <Tag>{AGENT.tag}</Tag>
                <h3 className="text-[17px] font-medium text-foreground">{AGENT.title}</h3>
                <p className="max-w-[58ch] text-[15px] leading-[1.5] text-muted-foreground">
                  {AGENT.body}
                </p>
                <div className="mt-auto flex flex-wrap items-center justify-between gap-3 pt-2">
                  <Arrow href={AGENT.link.href}>{AGENT.link.label}</Arrow>
                  <span className="text-xs text-muted-foreground">
                    <kbd className="rounded-sm border border-border px-1.5 py-0.5 font-mono">⌘J</kbd>{" "}
                    in the console
                  </span>
                </div>
              </div>
              <dl className="flex flex-col divide-y divide-border border-t border-border lg:border-l lg:border-t-0 lg:pl-6">
                {AGENT.asks.map((ask) => (
                  <div key={ask.title} className="flex flex-col gap-1 py-3 lg:py-4">
                    <dt className="text-sm font-medium text-foreground">{ask.title}</dt>
                    <dd className="text-sm leading-[1.4] text-muted-foreground">{ask.body}</dd>
                  </div>
                ))}
              </dl>
            </div>
            <div className="mx-5 border-t border-border" />
            <div className="grid grid-cols-1 gap-6 p-5 sm:grid-cols-3 sm:gap-0">
              {DOORS.map((door, i) => (
                <div
                  key={door.title}
                  className={cn(
                    "flex flex-col gap-3",
                    i > 0 && "border-t border-border pt-6 sm:border-t-0 sm:border-l sm:pt-0 sm:pl-6",
                    i < DOORS.length - 1 && "sm:pr-6",
                  )}
                >
                  <Tag>{door.tag}</Tag>
                  <h3 className="text-[15px] font-medium text-foreground">{door.title}</h3>
                  <p className="text-sm leading-[1.45] text-muted-foreground">{door.body}</p>
                  <div className="mt-auto pt-1">
                    <Arrow href={door.link.href}>{door.link.label}</Arrow>
                  </div>
                </div>
              ))}
            </div>
          </Cells>
        </section>

        <section className="flex flex-col gap-6">
          <SectionHead
            label="Why"
            title={
              <>
                <Mark>Why</Mark> Telmoni?
              </>
            }
            blurb="For developers who value open source and control over their data."
          />
          <dl className="border-t border-border">
            {WHY.map((reason) => (
              <div
                key={reason.title}
                className="grid gap-2 border-b border-border py-4 sm:grid-cols-[1fr_3fr] sm:gap-8"
              >
                <dt className="text-[15px] font-medium text-foreground">{reason.title}</dt>
                <dd className="text-sm leading-[1.5] text-muted-foreground">{reason.body}</dd>
              </div>
            ))}
          </dl>
        </section>

        <section className="flex flex-col gap-6">
          <SectionHead
            label="Get started"
            title={
              <>
                Start in <Mark>a minute</Mark>.
              </>
            }
            blurb="Sign up, create a project in the organization your first sign-in makes, make an API key and install the CLI. Or hand it to a coding agent."
          />
          <Cells className="bg-card">
            <div className="flex flex-col gap-6 p-5 lg:grid lg:grid-cols-[minmax(0,1fr)_minmax(0,2fr)] lg:gap-8">
              <div className="flex flex-col gap-4">
                <h3 className="text-[15px] font-medium text-foreground">By hand</h3>
                <p className="text-sm leading-[1.5] text-muted-foreground">
                  {"The console walks you through the organization and the project; the docs have the CLI's installer and the API's reference."}
                </p>
                <div className="mt-auto pt-1">
                  <WayIn signedIn={signedIn} align="start" />
                </div>
              </div>
              <div className="flex flex-col gap-4 border-t border-border pt-6 lg:border-t-0 lg:border-l lg:pt-0 lg:pl-8">
                <h3 className="text-[15px] font-medium text-foreground">With a coding agent</h3>
                <p className="text-sm leading-[1.5] text-muted-foreground">
                  The agent skill teaches Claude Code, Codex, Cursor and any agent that reads SKILL.md
                  to drive the CLI, call the API and build a webhook receiver.
                </p>
                <pre className="overflow-x-auto rounded-md border border-border bg-muted/40 p-3 text-xs leading-[1.6] text-foreground">
                  <code>{"/plugin marketplace add telmoni/skills\n/plugin install telmoni@telmoni"}</code>
                </pre>
                <p className="text-xs leading-[1.5] text-muted-foreground">
                  From any other agent, <code>npx skills add telmoni/skills</code>. Then ask it to set
                  this project up with Telmoni: the CLI, an API key and a webhook receiver.
                </p>
              </div>
            </div>
          </Cells>
        </section>

        <section className="flex flex-col gap-6">
          <SectionHead label="Questions" title="Questions and answers" />
          <div className="border-t border-border">
            {FAQ.map((entry) => (
              <details key={entry.question} className="group border-b border-border">
                <summary className="flex cursor-pointer list-none items-center justify-between gap-4 py-5 text-[15px] font-medium text-foreground [&::-webkit-details-marker]:hidden">
                  {entry.question}
                  <Plus aria-hidden="true" className="size-4 shrink-0 text-muted-foreground group-open:hidden" />
                  <Minus aria-hidden="true" className="hidden size-4 shrink-0 text-muted-foreground group-open:block" />
                </summary>
                <p className="max-w-[64ch] pb-5 text-sm leading-[1.5] text-muted-foreground">
                  {entry.answer}
                </p>
              </details>
            ))}
          </div>
        </section>
      </main>
    </PaperShell>
  );
}
