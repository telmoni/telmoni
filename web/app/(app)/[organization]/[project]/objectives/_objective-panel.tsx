"use client";

import { startTransition, useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Target } from "lucide-react";

import { useConsoleShell } from "@/components/console-shell";
import { EnumSelect } from "@/components/enum-select";
import { PageAction } from "@/components/page-action";
import {
  SidePanel,
  SidePanelBody,
  SidePanelFooter,
  SidePanelHeader,
  SidePanelRow,
} from "@/components/side-panel";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { amount, historyDays, type ConnectionChoice } from "@/lib/alerts";
import {
  FAST_BURN,
  NEW_OBJECTIVE,
  OBJECTIVES_HEADING,
  OBJECTIVE_DAYS,
  SLOW_BURN,
  agentsOf,
  objectiveInputOf,
  runDurationsOf,
  type ObjectiveDraft,
  type Promised,
} from "@/lib/objectives";
import type { History } from "@/lib/server/data";
import { RuleAggregation, RuleMeasure } from "@/lib/types/enums";
import { useAction } from "@/lib/use-action";
import { decimalTyped, wholeTyped } from "@/lib/typed-number";

import { useLanding } from "../alerts/_act";
import { Links, useOpening } from "../alerts/_alert-panel";
import { saveAlertAction } from "../alerts/actions";

// An objective is made and edited in the console's side panel, full over the
// content area, as an alert is: its agent, what it promises of the agent's
// runs and over how many days, how often a held breach is told again, and
// the connectors its notices go to. It is saved as the burn alert it is.

type PanelProps = {
  projectId: string;
  /** The objective's alert saved over; none for a new one. */
  ruleId: string | null;
  initial: ObjectiveDraft;
  connections: ConnectionChoice[] | null;
  /** Its agents' history: their names, and their runs' durations a latency may start from. */
  history: History | null;
};

type Opening = ReturnType<typeof useOpening>["opening"];

/** The panel, while it is open, in the console's content area. */
export function ObjectivePanelHost({
  opening,
  onClose,
  ...panel
}: PanelProps & { opening: Opening; onClose: () => void }) {
  const { element } = useConsoleShell();
  if (!opening) return null;
  const drawn = (
    <ObjectivePanel key={opening.at} {...panel} onClose={onClose} returnFocus={() => opening.returnTo} />
  );
  return element ? createPortal(drawn, element) : drawn;
}

/** The page's primary action: a new objective, any member's to make. */
export function NewObjective({
  projectId,
  connections,
  history,
}: {
  projectId: string;
  connections: ConnectionChoice[] | null;
  history: History | null;
}) {
  const { opening, open, close } = useOpening();
  return (
    <>
      <PageAction primary="New objective" onClick={(e) => open(e.currentTarget)}>
        <Target className="size-4" />
        New objective
      </PageAction>
      <ObjectivePanelHost
        opening={opening}
        onClose={close}
        projectId={projectId}
        ruleId={null}
        initial={NEW_OBJECTIVE}
        connections={connections}
        history={history}
      />
    </>
  );
}

const PROMISE_LABEL: Record<Promised, string> = {
  success: "Succeed",
  latency: "End inside a time",
};

function ObjectivePanel({
  projectId,
  ruleId,
  initial,
  connections,
  history,
  onClose,
  returnFocus,
}: PanelProps & { onClose: () => void; returnFocus: () => HTMLElement | null }) {
  const titleId = useId();
  const id = useId();
  const panel = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const [draft, setDraft] = useState<ObjectiveDraft>(initial);
  const { pending, error, setError, run } = useAction();
  const said = useRef<HTMLParagraphElement>(null);
  const { gone } = useLanding(OBJECTIVES_HEADING);

  useEffect(() => {
    if (error) said.current?.scrollIntoView?.({ block: "nearest" });
  }, [error]);

  const change = (next: ObjectiveDraft) => {
    setDraft(next);
    setError(null);
  };
  const agents = agentsOf(history);
  const durations = draft.promise === "latency" ? runDurationsOf(history, draft.agent) : null;
  const levels = durations
    ? (["p50", "p90", "p99"] as const).flatMap((level) => {
        const value = durations[level];
        return value === null ? [] : [{ level, ms: Math.round(value) }];
      })
    : [];

  function submit() {
    const checked = objectiveInputOf(draft);
    if (!checked.ok) {
      setError(checked.error);
      return;
    }
    run(
      async () => {
        const r = await saveAlertAction(projectId, ruleId, checked.input);
        return gone(r) ? { ok: true, close: false } : r.ok ? { ok: true, close: true } : r;
      },
      ({ close }) => close && startTransition(onClose),
    );
  }

  return (
    <SidePanel panel={panel} titleId={titleId} onClose={onClose} full initialFocus={field} returnFocus={returnFocus}>
      <form
        className="flex min-h-0 flex-1 flex-col"
        onSubmit={(e) => {
          e.preventDefault();
          if (!pending) submit();
        }}
      >
        <SidePanelHeader
          icon={Target}
          titleId={titleId}
          title={ruleId ? `Edit ${initial.name}` : "New objective"}
          onClose={onClose}
        >
          <p className="text-sm text-muted-foreground">
            A share of one agent&rsquo;s runs promised to succeed, or to end inside a time, over its
            days; the runs that miss it spend its error budget. It pages when its last hour spends
            the budget {FAST_BURN}&times; as fast as it lasts, and warns at {SLOW_BURN}&times; over its
            last six hours, through the project&rsquo;s feed and the connectors it links.
          </p>
        </SidePanelHeader>
        <SidePanelBody>
          <SidePanelRow label="Name" htmlFor={`${id}-name`}>
            <Input
              ref={field}
              id={`${id}-name`}
              value={draft.name}
              onChange={(e) => change({ ...draft, name: e.target.value })}
              placeholder="e.g. Support answers"
              autoComplete="off"
              disabled={pending}
              className="max-w-md"
            />
          </SidePanelRow>
          <SidePanelRow label="Agent" htmlFor={`${id}-agent`}>
            <Input
              id={`${id}-agent`}
              value={draft.agent}
              onChange={(e) => change({ ...draft, agent: e.target.value })}
              placeholder="e.g. support-bot"
              autoComplete="off"
              spellCheck={false}
              disabled={pending}
              list={agents.length > 0 ? `${id}-agents` : undefined}
              className="max-w-md"
            />
            {agents.length > 0 && (
              <datalist id={`${id}-agents`}>
                {agents.map((agent) => (
                  <option key={agent} value={agent} />
                ))}
              </datalist>
            )}
          </SidePanelRow>
          <SidePanelRow label="Its runs">
            <ToggleGroup
              type="single"
              variant="outline"
              size="sm"
              value={draft.promise}
              onValueChange={(promise) => promise && change({ ...draft, promise: promise as Promised })}
              aria-label="What its runs are promised to do"
              disabled={pending}
            >
              {(["success", "latency"] as const).map((promise) => (
                <ToggleGroupItem key={promise} value={promise}>
                  {PROMISE_LABEL[promise]}
                </ToggleGroupItem>
              ))}
            </ToggleGroup>
          </SidePanelRow>
          {draft.promise === "latency" && (
            <SidePanelRow label="Inside" htmlFor={`${id}-latency`}>
              <div className="flex max-w-md items-center gap-gutter">
                <Input
                  id={`${id}-latency`}
                  inputMode="numeric"
                  value={draft.latencyMs}
                  onChange={(e) => change({ ...draft, latencyMs: wholeTyped(e.target.value) })}
                  autoComplete="off"
                  disabled={pending}
                  aria-describedby={`${id}-latency-unit`}
                  className="w-36"
                />
                <span id={`${id}-latency-unit`} className="text-muted-foreground">
                  ms
                </span>
              </div>
              {levels.length > 0 && (
                <div className="mt-1.5 grid gap-1.5">
                  <p id={`${id}-history`} className="text-muted-foreground">
                    Its runs&rsquo; durations over the last {history ? historyDays(history) : 0} days:
                  </p>
                  <div className="flex flex-wrap gap-gutter">
                    {levels.map(({ level, ms }) => (
                      <Button
                        key={level}
                        type="button"
                        variant="outline"
                        size="sm"
                        disabled={pending}
                        aria-pressed={draft.latencyMs.trim() === String(ms)}
                        aria-describedby={`${id}-history`}
                        onClick={() => change({ ...draft, latencyMs: String(ms) })}
                      >
                        {level} · {amount(RuleMeasure.Duration, RuleAggregation.Avg, ms)}
                      </Button>
                    ))}
                  </div>
                  {draft.latencyMs.trim() && (
                    <p className="text-muted-foreground">Replaces {draft.latencyMs.trim()} ms, typed now.</p>
                  )}
                </div>
              )}
            </SidePanelRow>
          )}
          <SidePanelRow label="Promised" htmlFor={`${id}-target`}>
            <div className="flex max-w-md items-center gap-gutter">
              <Input
                id={`${id}-target`}
                inputMode="decimal"
                value={draft.target}
                onChange={(e) => change({ ...draft, target: decimalTyped(e.target.value) })}
                autoComplete="off"
                disabled={pending}
                aria-describedby={`${id}-target-unit`}
                className="w-28"
              />
              <span id={`${id}-target-unit`} className="text-muted-foreground">
                % of its runs
              </span>
            </div>
          </SidePanelRow>
          <SidePanelRow label="Over" htmlFor={`${id}-days`}>
            <EnumSelect
              id={`${id}-days`}
              value={String(draft.days)}
              options={OBJECTIVE_DAYS.map(String)}
              label={(days) => `The last ${days} days`}
              onChange={(days) =>
                change({ ...draft, days: OBJECTIVE_DAYS.find((d) => String(d) === days) ?? draft.days })
              }
              disabled={pending}
            />
          </SidePanelRow>
          <SidePanelRow label="Notify again" htmlFor={`${id}-renotify`}>
            <div className="flex max-w-md items-center gap-gutter">
              <span className="text-muted-foreground">Every</span>
              <Input
                id={`${id}-renotify`}
                inputMode="numeric"
                value={draft.renotifyMinutes}
                onChange={(e) => change({ ...draft, renotifyMinutes: wholeTyped(e.target.value) })}
                autoComplete="off"
                disabled={pending}
                aria-describedby={`${id}-renotify-unit ${id}-renotify-hint`}
                className="w-28"
              />
              <span id={`${id}-renotify-unit`} className="text-muted-foreground">
                minutes
              </span>
            </div>
            <p id={`${id}-renotify-hint`} className="text-muted-foreground">
              While its incident holds and nobody has acknowledged it; empty for never.
            </p>
          </SidePanelRow>
          <SidePanelRow label="Connectors">
            <Links
              id={id}
              links={draft.connectors}
              connections={connections}
              onChange={(connectors) => change({ ...draft, connectors })}
              disabled={pending}
            />
          </SidePanelRow>
          {error && (
            <p ref={said} className="py-2 text-sm text-destructive" role="alert">
              {error}
            </p>
          )}
        </SidePanelBody>
        <SidePanelFooter>
          <Button type="submit" disabled={pending}>
            {pending ? "Saving…" : ruleId ? "Save objective" : "Make objective"}
          </Button>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
        </SidePanelFooter>
      </form>
    </SidePanel>
  );
}
