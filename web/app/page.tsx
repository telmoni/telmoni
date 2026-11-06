import { Minus, Plus } from "lucide-react";

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
import { DOCS_URL } from "@/lib/site";

import { ForgetPersonalStorage } from "./_forget-personal-storage";
import { WayIn } from "./_way-in";

// No `metadata` here on purpose. `layout.tsx` templates a page title as
// `%s · Telmoni`, so a string set on the home route renders the product name
// twice; its `default` is already PRODUCT_NAME, which is what this route wants.

const docs = (path: string) => `${DOCS_URL}${path}`;

// What the product is, over the sentence the page makes, and the paragraph
// under it: one job, said once.
const KICKER = "Privacy-first, open-source monitoring for AI agents";
// Beside the kicker, so nobody takes the product for finished.
const STAGE = "Beta";
const CLAIM = "Know when your agents fail, stall, loop or overspend, and stop them at the cap you set.";
const PITCH =
  "Telmoni watches the agents you run, from a Python service to a Claude Code session, alerts you when one goes wrong, and shows you the run that did it. Prompts stay in your process unless you turn content on.";

// One way in per tab, each line the book's.
const INSTALL: readonly InstallTab[] = [
  { label: "pip", command: "pip install telmoni" },
  { label: "npm", command: "npm install telmoni" },
  { label: "cargo", command: "cargo add telmoni" },
  { label: "go", command: "go get github.com/telmoni/telmoni/sdk/go" },
  {
    label: "claude code",
    command:
      "claude plugin marketplace add telmoni/claude-code-observability-plugin && claude plugin install telmoni-observability@telmoni-plugins",
  },
  {
    label: "antigravity",
    command: "agy plugin install https://github.com/telmoni/antigravity-observability-plugin",
  },
];

type Pointer = { label: string; href: string };

// What Telmoni catches, a line each, each the alert or the guard that does it.
// Nothing is named here as running before it is built.
const CATCHES: { title: string; body: string; link: Pointer }[] = [
  {
    title: "A run that fails.",
    body: "The failures over an hour, a day or a week, or their share of runs, against the threshold you set.",
    link: { label: "Alerts", href: docs("/monitoring/alerts") },
  },
  {
    title: "A run that loops or stalls.",
    body: "The same tool called again and again, too many steps, or a run left open.",
    link: { label: "Loops and stalls", href: docs("/monitoring/alerts#loops-and-stalls") },
  },
  {
    title: "A run that overspends.",
    body: "Its cost read as its calls land, so the alert fires while the run is still going, and a monthly budget that says the day it will be reached.",
    link: { label: "Budgets", href: docs("/monitoring/alerts#a-budget") },
  },
  {
    title: "Agents that go quiet.",
    body: "Runs that stop arriving, read against the window before, so a nightly batch that didn't run opens an incident.",
    link: { label: "No data", href: docs("/monitoring/alerts#making-an-alert") },
  },
  {
    title: "And stops the runaway.",
    body: "The SDKs' guard caps a run's steps and spend inside your own process, with no call to us, and stops it at the cap.",
    link: { label: "The guard", href: docs("/api/sdks#the-guard") },
  },
];

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
      { label: "Antigravity", href: docs("/api/coding-agents#antigravity") },
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

const FAQ = [
  {
    question: "How do I get started?",
    answer:
      "Sign up, create a project, make an API key and paste the first run its dialog offers for your language; or install the Claude Code or Antigravity plugin and give it the key. Your agent's runs appear in the console within minutes, and your alerts read them from there.",
  },
  {
    question: "What does it store?",
    answer:
      "Metadata on every project: timings, statuses, counts, costs, and model and tool names. Content — prompts, completions, the model's thinking, tool payloads, images and files — is kept in full only on a project whose owner turned its content mode on, from code that sends it; anything else is dropped at the door.",
  },
  {
    question: "Can I self-host it?",
    answer:
      "Yes, the whole platform, under Apache-2.0: one Rust server and one console, on Docker Compose on one machine or the Helm chart on Kubernetes.",
  },
  {
    question: "Where do alerts go?",
    answer:
      "To the console's feed, Slack channels and signed webhooks, once each time an incident opens and once when it resolves, with the run that shows it; acknowledge or mute one straight from Slack.",
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

        <PaperSection title="What it catches">
          <ul className="flex max-w-[80ch] flex-col gap-4">
            {CATCHES.map((item) => (
              <PaperPoint key={item.title} title={item.title}>
                {item.body}{" "}
                <PaperLink
                  href={item.link.href}
                  className="text-foreground underline underline-offset-4 hover:no-underline"
                >
                  {item.link.label}
                </PaperLink>
              </PaperPoint>
            ))}
          </ul>
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

/** A link as a small outlined chip, as the stack lists them:
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
