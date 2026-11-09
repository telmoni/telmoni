import { Star } from "lucide-react";
import Link from "next/link";

import { JsonLd } from "@/components/json-ld";
import {
  BODY,
  ConsoleVideoPlaceholder,
  FeatureGrid,
  FeatureItem,
  LEAD,
  PaperHero,
  PaperLink,
  PaperSection,
  SdkVideoPlaceholder,
  StatsProof,
  SurfaceCard,
  SurfaceGrid,
  TerminalVideoPlaceholder,
} from "@/components/paper-page";
import { PaperShell } from "@/components/paper-shell";
import { Button } from "@/components/ui/button";
import { getServerSession } from "@/lib/server/session";
import { DOCS_URL, REPO_URL } from "@/lib/site";

import { ForgetPersonalStorage } from "./_forget-personal-storage";
import { TerminalInstallMenu } from "./_way-in";

// No `metadata` here on purpose. `layout.tsx` templates a page title as
// `%s · Telmoni`, so a string set on the home route renders the product name
// twice; its `default` is already PRODUCT_NAME, which is what this route wants.

const docs = (path: string) => `${DOCS_URL}${path}`;

const TITLE = "Privacy-first, open-source monitoring for AI agents";
// The core promise and privacy guarantee taking the place of regular body copy
const SUBTITLE =
  "Know when your agents fail, stall, loop or overspend, and stop them at the cap you set. Prompts stay in your process unless you turn content on.";

// Each entry names what goes wrong, what happens, and the page of the book
// that explains the mechanism.
const FEATURES: readonly {
  title: string;
  body: string;
  link: { label: string; href: string };
}[] = [
  {
    title: "Failing runs and calls",
    body: "See when errors climb on a model, tool or provider, and which run failed.",
    link: { label: "Alerts", href: docs("/monitoring/alerts") },
  },
  {
    title: "Loops and stalls",
    body: "Catch a run that repeats the same tool call or stops making progress before it burns through a session.",
    link: { label: "Loops and stalls", href: docs("/monitoring/alerts#loops-and-stalls") },
  },
  {
    title: "Spend and token spikes",
    body: "Set daily or monthly budgets per project or agent, and catch a run that costs far more than usual.",
    link: { label: "Budgets", href: docs("/monitoring/alerts#a-budget") },
  },
  {
    title: "Silent agents",
    body: "Know when a scheduled or background agent stops sending runs at all.",
    link: { label: "No data", href: docs("/monitoring/alerts#making-an-alert") },
  },
  {
    title: "Hard caps in your process",
    body: "Stop a run in the SDK when it reaches the step or spend ceiling you set.",
    link: { label: "The guard", href: docs("/api/sdks#the-guard") },
  },
  {
    title: "Private by default",
    body: "The SDK strips prompts, replies and tool arguments before export. The server drops any that arrive with content off.",
    link: { label: "Field inventory", href: docs("/api/field-inventory") },
  },
  {
    title: "Content under your control",
    body: "Capture content only when both your SDK and project allow it, with secret scrubbing and a separate encryption key per project.",
    link: { label: "Content mode", href: docs("/api/sdks") },
  },
  {
    title: "A record of access",
    body: "End-user ids are hashed per project. Reads and changes are recorded in the audit log.",
    link: { label: "Read the field inventory", href: docs("/api/field-inventory") },
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
        <PaperHero title={TITLE}>
          <p className={LEAD}>{SUBTITLE}</p>
        </PaperHero>

        <PaperSection id="surfaces" className="pt-0 lg:pt-0">
          <SurfaceGrid>
            <SurfaceCard
              title="Console"
              description="Runs and the steps inside them, live alerts to Slack and webhooks, spend and budgets, and audit logs."
              action={
                signedIn ? (
                  <Button asChild size="sm" variant="filled">
                    <Link href="/console">Console</Link>
                  </Button>
                ) : (
                  <Button asChild size="sm" variant="filled">
                    {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
                    <a href="/auth/signup">Sign up</a>
                  </Button>
                )
              }
              preview={<ConsoleVideoPlaceholder />}
            />
            <SurfaceCard
              title="Terminal"
              description="Authenticate from your terminal or CI with the Telmoni CLI, and record Claude Code, Codex and Antigravity sessions with their plugins."
              action={<TerminalInstallMenu />}
              preview={<TerminalVideoPlaceholder />}
            />
            <SurfaceCard
              title="SDK"
              description="Drive Telmoni from Python, TypeScript, Go, and Rust with native OpenTelemetry export and privacy guards."
              action={
                <Button asChild size="sm" variant="filled">
                  <PaperLink href={docs("/api/sdks")}>Read docs</PaperLink>
                </Button>
              }
              preview={<SdkVideoPlaceholder />}
            />
          </SurfaceGrid>
        </PaperSection>

        <PaperSection
          title="What is Telmoni?"
          className="[&_[data-component=features]]:gap-gutter"
        >
          <p className={LEAD}>
            Telmoni is an open-source monitoring platform for AI agents. Follow runs,
            catch failures and control spend, with prompts kept in your process by default.{" "}
            <PaperLink href={DOCS_URL} className="text-foreground underline underline-offset-4 hover:no-underline">
              Read the docs.
            </PaperLink>
          </p>
          <FeatureGrid>
            {FEATURES.map((item) => (
              <FeatureItem
                key={item.title}
                title={item.title}
                description={
                  <>
                    {item.body}{" "}
                    <PaperLink
                      href={item.link.href}
                      className="text-foreground underline underline-offset-4 hover:no-underline"
                    >
                      {item.link.label}
                    </PaperLink>
                  </>
                }
              />
            ))}
          </FeatureGrid>
        </PaperSection>

        <PaperSection title="Open source">
          <p className={`max-w-[60ch] ${BODY}`}>
            With an <span className="text-foreground">Apache-2.0</span> license,{" "}
            <span className="text-foreground">4 native SDKs</span>, and{" "}
            <span className="text-foreground">OpenTelemetry</span> ingest, Telmoni is built in the
            open so you can run the whole platform on your own infrastructure: one Rust server and
            one console, on Docker Compose or Kubernetes.
          </p>
          <StatsProof
            stats={[
              { label: "License", value: "Apache-2.0" },
              { label: "Native SDKs", value: "4" },
              { label: "Standard ingest", value: "OTLP" },
            ]}
            note={
              REPO_URL ? (
                <>
                  <PaperLink
                    href={`${REPO_URL}/blob/main/LICENSE`}
                    className="text-foreground underline underline-offset-4 hover:no-underline"
                  >
                    Apache-2.0 licensed
                  </PaperLink>
                  . Built in the open by{" "}
                  <PaperLink
                    href={REPO_URL}
                    className="text-foreground underline underline-offset-4 hover:no-underline"
                  >
                    Telmoni
                  </PaperLink>{" "}
                  and{" "}
                  <PaperLink
                    href={`${REPO_URL}/graphs/contributors`}
                    className="text-foreground underline underline-offset-4 hover:no-underline"
                  >
                    contributors
                  </PaperLink>
                  .
                </>
              ) : (
                "Apache-2.0 licensed. Built in the open by Telmoni and contributors."
              )
            }
            action={
              <>
                {REPO_URL && (
                  <Button asChild variant="filled" size="sm" className="gap-2">
                    <PaperLink href={REPO_URL}>
                      <Star className="size-3.5 fill-current" aria-hidden="true" />
                      <span>Star on GitHub</span>
                    </PaperLink>
                  </Button>
                )}
                <Button
                  asChild
                  variant="secondary"
                  size="sm"
                  className="border-transparent bg-muted text-foreground hover:bg-muted/80"
                >
                  <PaperLink href={docs("/contributing/introduction")}>
                    Contribute
                  </PaperLink>
                </Button>
              </>
            }
          />
        </PaperSection>

      </main>
    </PaperShell>
  );
}
