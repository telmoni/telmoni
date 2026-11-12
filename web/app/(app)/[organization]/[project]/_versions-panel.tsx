"use client";

import { GitCompareArrows } from "lucide-react";
import { createContext, useContext, useEffect, useId, useRef, useState, type ReactNode } from "react";

import { DataTable, THead, Td, Th } from "@/components/data-table";
import { EnumSelect } from "@/components/enum-select";
import { LocalTime } from "@/components/local-time";
import { readRoute } from "@/components/route-read";
import { SidePanel, SidePanelBody, SidePanelHeader, usePanelOpening } from "@/components/side-panel";
import { PeekNote, SignInAgain, startRead } from "@/components/traces/peek-read";
import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { isOneOf } from "@/lib/one-of";
import type { RouteRead } from "@/lib/route-read";
import { formatScoreValue } from "@/lib/scores";
import { formatCost, formatCount, formatDuration } from "@/lib/traces";
import { ChangeKind } from "@/lib/types/enums";
import { cn } from "@/lib/utils";
import {
  BY_LABEL,
  VERSIONS_BY,
  VersionsSchema,
  deltaWords,
  deltas,
  environmentLabel,
  failedShare,
  formatShare,
  versionsReadHref,
  type DeltaKind,
  type Versions,
  type VersionsBy,
} from "@/lib/versions";

// An agent's versions side by side, in the console's side panel beside the
// Agents page and on the agent's own: its releases, its models or one
// prompt's versions, a row each of the traces that carried it, each number
// over its change from the version first seen before it. Read once drawn,
// and again as a tab or a picker moves.

const readVersions = (url: string, signal: AbortSignal) => readRoute(url, VersionsSchema, { signal });

/** Opens the page's one panel on an agent, from the button pressed. */
const OpenVersionsContext = createContext<((from: HTMLElement, agent: string) => void) | null>(null);

/** The Agents table's one panel: a row's button opens it on that row's
 *  agent, in place of whichever stood there. */
export function VersionsHost({
  projectId,
  days,
  children,
}: {
  projectId: string;
  /** The days the Agents page read, which the panel names until its own read lands. */
  days: number;
  children: ReactNode;
}) {
  const { opening, open, close, place } = usePanelOpening<string>();
  return (
    <OpenVersionsContext.Provider value={open}>
      {children}
      {place(
        opening && (
          <VersionsPanel
            key={opening.at}
            projectId={projectId}
            agent={opening.of}
            days={days}
            onClose={close}
            returnFocus={() => opening.returnTo}
          />
        ),
      )}
    </OpenVersionsContext.Provider>
  );
}

/** A row's button, which opens the table's panel on its agent. */
export function VersionsOpener({ agent }: { agent: string }) {
  const open = useContext(OpenVersionsContext);
  if (!open) {
    throw new Error("VersionsOpener requires a <VersionsHost> ancestor");
  }
  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      aria-label={`Versions of ${agent}`}
      onClick={(e) => open(e.currentTarget, agent)}
    >
      <GitCompareArrows />
      Versions
    </Button>
  );
}

/** A number's change from the row below, under it: coloured where a rise is
 *  for the worse, plain for a score, since which way is better is not known. */
function Delta({ delta, kind }: { delta: number | null | undefined; kind: DeltaKind }) {
  if (delta == null) return null;
  const words = deltaWords(kind, delta);
  if (words === null) return <span className="block text-xs text-muted-foreground">No change</span>;
  const judged = kind !== "score";
  return (
    <span
      className={cn(
        "block text-xs",
        judged ? (delta > 0 ? "text-destructive" : "text-brand-positive") : "text-muted-foreground",
      )}
    >
      {words}
    </span>
  );
}

const NUMBER = "text-right whitespace-nowrap tabular-nums";

function VersionsTable({ answer }: { answer: Versions }) {
  const changes = deltas(answer.versions);
  const { one, column } = BY_LABEL[answer.by];
  return (
    <DataTable>
      <THead>
        <Th>{column}</Th>
        <Th className="text-right">Traces</Th>
        <Th className="text-right">Failed</Th>
        <Th className="text-right">Median</Th>
        <Th className="text-right">p95</Th>
        <Th className="text-right">Median cost</Th>
        {answer.scores.map((name) => (
          <Th key={name} className="text-right">
            {name}
          </Th>
        ))}
      </THead>
      <tbody>
        {answer.versions.map((version, at) => {
          const change = changes[at];
          const share = failedShare(version);
          return (
            <tr key={version.value}>
              <Td>
                {version.value === "" ? (
                  <span className="text-muted-foreground">(none)</span>
                ) : (
                  <span className="block max-w-64 truncate font-medium" title={version.value}>
                    {version.value}
                  </span>
                )}
                <span className="block text-xs whitespace-nowrap text-muted-foreground">
                  {version.value === "" ? (
                    `Traces that carried no ${one}`
                  ) : (
                    <>
                      First seen <LocalTime iso={version.first_at} />
                    </>
                  )}
                </span>
              </Td>
              <Td className={NUMBER}>{formatCount(version.traces)}</Td>
              <Td className={NUMBER}>
                {share === null ? "—" : formatShare(share)}
                <Delta delta={change?.failed} kind="failed" />
              </Td>
              <Td className={NUMBER}>
                {version.p50_ms === null ? "—" : formatDuration(version.p50_ms)}
                <Delta delta={change?.p50_ms} kind="duration" />
              </Td>
              <Td className={NUMBER}>
                {version.p95_ms === null ? "—" : formatDuration(version.p95_ms)}
                <Delta delta={change?.p95_ms} kind="duration" />
              </Td>
              <Td className={NUMBER}>
                {version.median_spend === null ? "—" : formatCost(version.median_spend)}
                <Delta delta={change?.median_spend} kind="cost" />
              </Td>
              {answer.scores.map((name) => {
                const score = version.scores.find((given) => given.name === name);
                return (
                  <Td key={name} className={NUMBER}>
                    {score ? formatScoreValue(score.mean) : "—"}
                    <Delta delta={change?.scores[name]} kind="score" />
                  </Td>
                );
              })}
            </tr>
          );
        })}
      </tbody>
    </DataTable>
  );
}

/** What the panel says where a read did not land. */
function Failed({ read, retry }: { read: Exclude<RouteRead<unknown>, { kind: "ok" }>; retry: () => void }) {
  if (read.kind === "forbidden") {
    return (
      <PeekNote title="This project's traces are not yours to read.">
        Ask an owner of the project for a role that reads them.
      </PeekNote>
    );
  }
  if (read.kind === "signed-out") {
    return (
      <PeekNote title="Your session has ended.">
        <SignInAgain /> to read them.
      </PeekNote>
    );
  }
  return (
    <PeekNote title="The versions could not be read.">
      <button type="button" onClick={retry} className="cursor-pointer underline underline-offset-4">
        Try again
      </button>{" "}
      in a moment.
    </PeekNote>
  );
}

const lastDays = (days: number) => (days === 1 ? "the last day" : `the last ${days} days`);

/** An agent's versions over `days`, as both of their places draw them: what
 *  is picked, the read it asks for, the days that read answered, and the
 *  pickers and rows that come of it. */
function useVersions(projectId: string, agent: string, days: number) {
  const id = useId();
  const [by, setBy] = useState<VersionsBy>(ChangeKind.Release);
  // Left to the server until one is picked: the busiest, and its most named prompt.
  const [environment, setEnvironment] = useState<string | undefined>(undefined);
  const [prompt, setPrompt] = useState<string | undefined>(undefined);
  const [attempt, setAttempt] = useState(0);
  const [last, setLast] = useState<{ key: string; read: RouteRead<Versions> } | null>(null);
  // The last answer that landed, which the pickers and the days stay drawn
  // from while the next is read, and where one fails.
  const [drawn, setDrawn] = useState<Versions | null>(null);

  const url = versionsReadHref(projectId, {
    agent,
    by,
    environment,
    prompt: by === ChangeKind.Prompt ? prompt : undefined,
    days,
  });
  const key = `${attempt} ${url}`;
  useEffect(
    () =>
      startRead(url, readVersions, (read) => {
        setLast({ key, read });
        if (read.kind === "ok") setDrawn(read.value);
      }),
    [url, key],
  );

  const read = last?.key === key ? last.read : null;
  const span = lastDays(drawn?.days ?? days);
  const environments = drawn?.environments ?? [];
  // What is picked shows at once, ahead of the read that answers it.
  const shownEnvironment = environment ?? drawn?.environment;
  const prompts = by === ChangeKind.Prompt ? (drawn?.prompts ?? []) : [];
  const shownPrompt = by === ChangeKind.Prompt ? (prompt ?? drawn?.prompt ?? null) : null;

  let body: ReactNode;
  if (read === null) {
    body = <PeekNote>Reading its versions&hellip;</PeekNote>;
  } else if (read.kind !== "ok") {
    body = <Failed read={read} retry={() => setAttempt((n) => n + 1)} />;
  } else if (read.value.versions.length === 0) {
    const here = environmentLabel(read.value.environment);
    body = (
      <PeekNote title={`Nothing to set side by side in ${span}.`}>
        {by === ChangeKind.Prompt
          ? `None of its traces in ${here} named a prompt.`
          : `None of its traces had a span in ${here}.`}
      </PeekNote>
    );
  } else {
    body = (
      <div className="grid gap-gutter">
        <VersionsTable answer={read.value} />
        <p className="text-sm text-muted-foreground">
          A row is the traces whose spans in {environmentLabel(read.value.environment)} carried it over {span}, the
          newest first seen first: first seen in those days, which is not when it was deployed. The figure under a
          number is its plain difference from the row below, and nothing is inferred from it.
          {by === ChangeKind.Model && " A trace counts under every model it called."}
          {read.value.more && " More were seen than are shown: the oldest are left out."}
        </p>
      </div>
    );
  }

  return {
    span,
    busy: read === null,
    view: (
      <>
        <div className="flex flex-wrap items-center gap-gutter">
          <Tabs value={by} onValueChange={(next) => isOneOf(VERSIONS_BY, next) && setBy(next)}>
            <TabsList aria-label="What its traces are split by">
              {VERSIONS_BY.map((each) => (
                <TabsTrigger key={each} value={each}>
                  {BY_LABEL[each].tab}
                </TabsTrigger>
              ))}
            </TabsList>
          </Tabs>
          {environments.length > 1 && (
            // By its place in the list: the environment with no name is an
            // empty word, which a select's item cannot be.
            <EnumSelect
              id={`${id}-environment`}
              ariaLabel="Environment"
              className="w-auto max-w-64"
              value={String(Math.max(0, environments.findIndex((each) => each.value === shownEnvironment)))}
              options={environments.map((_, at) => String(at))}
              label={(at) => environmentLabel(environments[Number(at)]?.value ?? "")}
              onChange={(at) => {
                setEnvironment(environments[Number(at)]?.value);
                // Each environment's traces name their own prompts.
                setPrompt(undefined);
              }}
            />
          )}
          {prompts.length > 0 && shownPrompt !== null && (
            <EnumSelect
              id={`${id}-prompt`}
              ariaLabel="Prompt"
              className="w-auto max-w-64"
              value={shownPrompt}
              options={
                prompts.some((each) => each.value === shownPrompt)
                  ? prompts.map((each) => each.value)
                  : [shownPrompt, ...prompts.map((each) => each.value)]
              }
              label={(name) => name}
              onChange={setPrompt}
            />
          )}
        </div>
        {body}
      </>
    ),
  };
}

/** An agent's versions on its own page, over the page's days. */
export function AgentVersions({ projectId, agent, days }: { projectId: string; agent: string; days: number }) {
  const { busy, view } = useVersions(projectId, agent, days);
  return (
    <div className="grid gap-gutter" aria-busy={busy}>
      {view}
    </div>
  );
}

function VersionsPanel({
  projectId,
  agent,
  days,
  onClose,
  returnFocus,
}: {
  projectId: string;
  agent: string;
  days: number;
  onClose: () => void;
  returnFocus: () => HTMLElement | null;
}) {
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const { span, busy, view } = useVersions(projectId, agent, days);
  return (
    <SidePanel panel={panel} titleId={titleId} onClose={onClose} returnFocus={returnFocus}>
      <SidePanelHeader icon={GitCompareArrows} titleId={titleId} title={agent} onClose={onClose}>
        <p className="text-sm text-muted-foreground">
          Its releases, its models and each prompt&rsquo;s versions side by side over {span}.
        </p>
      </SidePanelHeader>
      <SidePanelBody>
        <div className="grid gap-gutter py-gutter" aria-busy={busy}>
          {view}
        </div>
      </SidePanelBody>
    </SidePanel>
  );
}
