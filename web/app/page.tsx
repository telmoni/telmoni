import Link from "next/link";

import { JsonLd } from "@/components/json-ld";
import { Mark } from "@/components/mark";
import { PaperShell } from "@/components/paper-shell";
import { SplashHero } from "@/components/splash-hero";
import { Button } from "@/components/ui/button";
import { getServerSession } from "@/lib/server/session";
import { DOCS_URL, REPO_URL, SKILLS_URL } from "@/lib/site";
import { cn } from "@/lib/utils";

// No `metadata` here on purpose. `layout.tsx` templates a page title as
// `%s · Telmoni`, so a string set on the home route renders the product name
// twice; its `default` is already PRODUCT_NAME, which is what this route wants.

const docs = (path: string) => `${DOCS_URL}${path}`;

// The line above the claim: what this is, and where it stands.
const CAPTIONS = ["Open source · Apache-2.0", "Not launched · Built in the open"];

// The sentence the page makes, the phrase it leans on, and the paragraph
// under it: the product as it is being built, in the words its roadmap uses
// for its three goals.
const CLAIM = "Know when your agents stall, loop or overspend, without sending us a single prompt.";
const HIGHLIGHT = "stall, loop or overspend";
const PITCH =
  "Every run and the steps inside it, every model and tool call with its tokens, latency and cost, the bounds each agent keeps, and the alert when one breaks them. Prompts and completions stay out unless an organization owner turns them on for a project.";

// True today, and the sentence that goes when the first run is recorded.
const STATUS =
  "Telmoni has not launched. What runs today is the foundation — organizations and roles, API keys, the audit log, notifications and the console agent — and the telemetry is built on it in the open.";

// What the product is for, one cell per goal, in the order they are built:
// a small motion that says it, a line, then three points.
const GOALS = [
  {
    title: "Telemetry for agents",
    glyph: "spans",
    lead: "Every run and the steps inside it, from any framework.",
    points: [
      "Runs, steps, model and tool calls over OpenTelemetry's own protocol, or a curl from a crontab",
      "Tokens, latency, cost and failures, per run, per project, per end customer",
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
      "Slack, Discord, webhooks, mail and PagerDuty",
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

// The open platform, four cells of links into the documentation or the
// source.
const OPEN: { title: string; blurb: string; links: Pointer[] }[] = [
  {
    title: "Self-host",
    blurb: "The whole platform on your own infrastructure. You bring the identity provider, the mail transport and the model.",
    links: [
      { label: "Docker Compose", href: docs("/self-host/docker-compose") },
      { label: "Kubernetes, with Helm", href: docs("/self-host/kubernetes") },
      { label: "Production hardening", href: docs("/self-host/production") },
      { label: "The agent on your own model", href: docs("/self-host/agent") },
    ],
  },
  {
    title: "Apache-2.0",
    blurb: "Every feature, with no limits and nothing held back: one Rust binary behind a Next.js console.",
    links: [
      ...(REPO_URL ? [{ label: "Read the source", href: REPO_URL }] : []),
      { label: "Fork, modify, contribute", href: docs("/contributing/introduction") },
      { label: "How it is built", href: docs("/contributing/developing") },
    ],
  },
  {
    title: "Documented",
    blurb: "The docs describe what is implemented today, as it is, and every error the API answers has a page.",
    links: [
      { label: "The docs", href: DOCS_URL },
      { label: "API reference", href: docs("/api/reference") },
      { label: "Errors", href: docs("/errors") },
      { label: "The SDKs", href: docs("/api/sdks") },
    ],
  },
  {
    title: "Security as mechanism",
    blurb: "Isolation the database enforces, an audit record in the same transaction as the change, and deletion that ends.",
    links: [
      { label: "Tenant isolation", href: docs("/legal/security/#tenant-isolation") },
      { label: "Secrets", href: docs("/legal/security/#secrets") },
      { label: "Audit", href: docs("/legal/security/#audit") },
      { label: "Data deletion", href: docs("/legal/security/#data-deletion") },
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
    body: "Metadata by default, and the content switch is the organization owner's, per project: on, off or sealed.",
  },
  {
    title: "Open source",
    body: "Apache-2.0, every feature. Self-host it on a laptop or a cluster, or let somebody run it for you.",
  },
  {
    title: "One binary",
    body: "The server is one Rust binary and the console one Next.js app, with a database and a cache beside them.",
  },
  {
    title: "OpenTelemetry native",
    body: "Our SDKs, any instrumentation that speaks the GenAI conventions, or a curl from a crontab.",
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
    question: "Has it launched?",
    answer:
      "Not yet. What runs today is the foundation: organizations and roles, API keys, the audit log, notifications, the console agent and the CLI. Nothing records a run yet; the telemetry is being built on it in the open.",
  },
  {
    question: "What does it store?",
    answer:
      "Metadata: timings, statuses, counts, model and tool names, costs. Prompts, completions and tool payloads are not sent by the SDKs and are dropped at the door, unless an organization owner turns content on for a project.",
  },
  {
    question: "Can I self-host it?",
    answer:
      "Yes, the whole platform: Docker Compose on one machine, or the Helm chart on Kubernetes. You bring the identity provider, the mail transport and the model.",
  },
  {
    question: "How do I get started?",
    answer:
      "Sign up, create an organization and a project, make an API key and install the CLI; or install the agent skill and let a coding agent do it.",
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

// A goal's motion, in a cell's corner: spans arriving one under another, a
// schedule's pulse, and content masked while the lines around it stay.
// Decorative, and still under reduced motion.
function Glyph({ kind }: { kind: "spans" | "pulse" | "redact" }) {
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
              className="absolute inline-flex size-full animate-ping [border-radius:9999px] bg-foreground/40 motion-reduce:animate-none"
              style={{ animationDelay: `${i * 500}ms` }}
            />
            <span className="relative inline-flex size-2 [border-radius:9999px] bg-foreground/70" />
          </span>
        ))}
      </span>
    );
  }
  return (
    <span aria-hidden="true" className="flex w-16 flex-col gap-1">
      <span className="block h-1 w-3/4 rounded-sm bg-foreground/60" />
      <span className="relative block h-1 w-full overflow-hidden rounded-sm bg-foreground/25">
        <span className="redact absolute inset-0 bg-foreground" />
      </span>
      <span className="block h-1 w-1/2 rounded-sm bg-foreground/60" />
    </span>
  );
}

// The small mono line a product site sets above a heading or in a cell's
// corner: a number, a tag.
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
    <span className="rounded-sm border border-border px-1.5 py-0.5 text-[11px] text-muted-foreground">
      {children}
    </span>
  );
}

// A section opens with its number, its heading with one phrase marked, and
// a line under it.
function SectionHead({
  number,
  title,
  blurb,
}: {
  number: string;
  title: React.ReactNode;
  blurb?: string;
}) {
  return (
    <div className="rise-in flex flex-col gap-3">
      <Label>{number}</Label>
      <h2 className="text-[28px] font-semibold leading-[1.15] tracking-tight text-foreground sm:text-[32px]">
        {title}
      </h2>
      {blurb && <p className="max-w-[64ch] text-[15px] leading-[1.5] text-muted-foreground">{blurb}</p>}
    </div>
  );
}

// A hairline grid: the cells share one rule, as a product site draws them.
// ⚠ Each cell draws the rule itself, a one-pixel ring, rather than the grid
// showing its ink through a one-pixel gap: an uneven split — a third and two
// thirds — lands the gap on a fraction of a pixel, which the browser paints
// as two, and the rule read doubled. A ring sits on the cell's own edge, so
// it is one pixel whatever the columns measure, and the frame clips the
// rings on the outside.
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

function Cell({ className, children }: { className?: string; children: React.ReactNode }) {
  return (
    <div className={cn("flex flex-col gap-3 bg-card p-5 ring-1 ring-border", className)}>
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
            number="01 · What it is for"
            title={
              <>
                Telemetry, monitoring, <Mark>privacy</Mark>.
              </>
            }
            blurb="Built in this order on the foundation below, for agents in production: a service, a nightly batch, a coding session, a graph."
          />
          {/* The cells share their rows — number, title, lead, list — so the
              rule above each list is one line across the row, where the
              longest lead ends, rather than three at three heights. */}
          <Cells className="sm:grid-cols-3">
            {GOALS.map((goal, i) => (
              <Cell key={goal.title} className="row-span-4 grid grid-rows-subgrid">
                <div className="flex items-start justify-between gap-4">
                  <Label>0{i + 1}</Label>
                  <Glyph kind={goal.glyph} />
                </div>
                <h3 className="text-[15px] font-medium text-foreground">{goal.title}</h3>
                <p className="text-sm leading-[1.5] text-muted-foreground">{goal.lead}</p>
                <ul className="mt-1 flex flex-col gap-2 border-t border-border pt-3 text-sm leading-[1.4] text-muted-foreground">
                  {goal.points.map((point) => (
                    <li key={point} className="flex gap-2">
                      <span aria-hidden="true" className="mt-[0.55em] size-1 shrink-0 rounded-full bg-muted-foreground" />
                      {point}
                    </li>
                  ))}
                </ul>
              </Cell>
            ))}
          </Cells>
        </section>

        <section className="flex flex-col gap-6">
          <SectionHead
            number="02 · Open source"
            title={
              <>
                <Mark>Open platform.</Mark> Open source.
              </>
            }
            blurb="The whole platform is Apache-2.0: the server, the console and the docs it serves, the CLI and the SDKs, and the agent skills. Nothing is held back."
          />
          <Cells className="sm:grid-cols-2">
            {OPEN.map((box) => (
              <Cell key={box.title}>
                <h3 className="text-[15px] font-medium text-foreground">{box.title}</h3>
                <p className="text-sm leading-[1.5] text-muted-foreground">{box.blurb}</p>
                <ul className="mt-auto flex flex-wrap gap-2 pt-1">
                  {box.links.map((link) => (
                    <li key={link.href}>
                      <Chip href={link.href}>{link.label}</Chip>
                    </li>
                  ))}
                </ul>
              </Cell>
            ))}
          </Cells>
        </section>

        <section className="flex flex-col gap-6">
          <SectionHead
            number="03 · Developers and agents"
            title={
              <>
                Work from the console or a terminal, or <Mark>send an agent</Mark>.
              </>
            }
            blurb="Work in the console or from a terminal. The console agent answers about your project with citations; the skill, the CLI and the API connect coding agents to Telmoni."
          />
          <Cells className="sm:grid-cols-3">
            {/* The agent's cell spans the row and splits in two at the wide
                breakpoint, the three things it finds beside the text, so
                nothing is left blank under it. */}
            <Cell className="gap-4 sm:col-span-3 lg:grid lg:grid-cols-[1.2fr_1fr] lg:gap-8">
              <div className="flex flex-col gap-4">
                <div className="flex items-center justify-between">
                  <Label>00</Label>
                  <Tag>{AGENT.tag}</Tag>
                </div>
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
            </Cell>
            {DOORS.map((door, i) => (
              <Cell key={door.title}>
                <div className="flex items-center justify-between">
                  <Label>0{i + 1}</Label>
                  <Tag>{door.tag}</Tag>
                </div>
                <h3 className="text-[15px] font-medium text-foreground">{door.title}</h3>
                <p className="text-sm leading-[1.45] text-muted-foreground">{door.body}</p>
                <div className="mt-auto pt-1">
                  <Arrow href={door.link.href}>{door.link.label}</Arrow>
                </div>
              </Cell>
            ))}
          </Cells>
        </section>

        <section className="flex flex-col gap-6">
          <SectionHead
            number="04 · Why"
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
            number="05 · Get started"
            title={
              <>
                Start in <Mark>a minute</Mark>.
              </>
            }
            blurb="Sign up, create an organization and a project, make an API key and install the CLI. Or hand it to a coding agent."
          />
          <Cells className="lg:grid-cols-[minmax(0,1fr)_minmax(0,2fr)]">
            <Cell className="gap-4">
              <h3 className="text-[15px] font-medium text-foreground">By hand</h3>
              <p className="text-sm leading-[1.5] text-muted-foreground">
                {"The console walks you through the organization and the project; the docs have the CLI's installer and the API's reference."}
              </p>
              <div className="mt-auto pt-1">
                <WayIn signedIn={signedIn} align="start" />
              </div>
            </Cell>
            <Cell className="gap-4">
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
            </Cell>
          </Cells>
        </section>

        <section className="flex flex-col gap-6">
          <SectionHead number="06 · Questions" title="Questions and answers" />
          <div className="border-t border-border">
            {FAQ.map((entry) => (
              <details key={entry.question} className="group border-b border-border">
                <summary className="flex cursor-pointer list-none items-center justify-between gap-4 py-5 text-[15px] font-medium text-foreground [&::-webkit-details-marker]:hidden">
                  {entry.question}
                  <span
                    aria-hidden="true"
                    className="relative size-4 shrink-0 text-muted-foreground before:absolute before:left-0 before:top-1/2 before:h-px before:w-4 before:-translate-y-1/2 before:bg-current after:absolute after:left-1/2 after:top-0 after:h-4 after:w-px after:-translate-x-1/2 after:bg-current after:transition-transform group-open:after:scale-y-0"
                  />
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
