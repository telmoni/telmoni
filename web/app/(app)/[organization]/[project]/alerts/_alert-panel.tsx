"use client";

import { unstable_rethrow } from "next/navigation";
import { startTransition, useEffect, useId, useRef, useState, useTransition } from "react";
import { createPortal } from "react-dom";
import { BellPlus, BellRing } from "lucide-react";

import { useConsoleShell } from "@/components/console-shell";
import { PageAction } from "@/components/page-action";
import {
  SidePanel,
  SidePanelFooter,
  SidePanelHeader,
  SidePanelRow,
} from "@/components/side-panel";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
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
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import {
  ALERTS_HEADING,
  ALERT_KINDS,
  DAYS,
  FILTER_LABEL,
  FILTER_STATUSES,
  KIND_PLURAL,
  CONNECTION_STATUS_LABEL,
  LEVEL_LABEL,
  LINKS_MAX,
  MEASURE_LABEL,
  MISSING_DATA_LABEL,
  NEW_ALERT,
  OFFSETS,
  OPERATOR_LABEL,
  UNIT_SUFFIX,
  WINDOW_LABEL,
  aggregationLabel,
  aggregationsFor,
  browserOffset,
  filterFieldsFor,
  filtersStatus,
  incidentKindsLabel,
  inputOf,
  measuresFor,
  offsetWords,
  takesWarning,
  unitOf,
  withKind,
  withMeasure,
  type AlertDraft,
  type AlertKind,
  type ConnectionChoice,
  type LinkDraft,
} from "@/lib/alerts";
import { STATUS_LABEL } from "@/lib/traces";
import {
  MissingData,
  RuleAggregation,
  RuleLevel,
  RuleMeasure,
  RuleOperator,
  RuleWindow,
  SpanStatus,
} from "@/lib/types/enums";

import { useLanding } from "./_act";
import { saveAlertAction } from "./actions";

// An alert is made and edited in the console's side panel, full over the
// content area, as a key is minted (`api-keys/_manage.tsx`): its name, what
// it reads and how, its thresholds, what a window with no span means, how
// often a held breach is told again, and the connectors its notices go to.

/** One opening of the panel: counted, so asking again draws it afresh, and
 *  the control the focus goes back to as it shuts. */
type Opening = { at: number; returnTo: HTMLElement | null };

/** The panel's opening, and the two ways to change it. */
export function useOpening() {
  const [opening, setOpening] = useState<Opening | null>(null);
  const openings = useRef(0);
  return {
    opening,
    open: (returnTo: HTMLElement | null) => setOpening({ at: ++openings.current, returnTo }),
    close: () => setOpening(null),
  };
}

type PanelProps = {
  projectId: string;
  /** The alert saved over; none for a new one. */
  ruleId: string | null;
  initial: AlertDraft;
  /** The project's connections; none where they could not be read. */
  connections: ConnectionChoice[] | null;
};

/** The panel, while it is open, in the console's content area. */
export function AlertPanelHost({
  opening,
  onClose,
  ...panel
}: PanelProps & { opening: Opening | null; onClose: () => void }) {
  const { element } = useConsoleShell();
  if (!opening) return null;
  const drawn = (
    <AlertPanel
      key={opening.at}
      {...panel}
      onClose={onClose}
      returnFocus={() => opening.returnTo}
    />
  );
  return element ? createPortal(drawn, element) : drawn;
}

/** The page's primary action: a new alert, any member's to make. */
export function NewAlert({
  projectId,
  connections,
}: {
  projectId: string;
  connections: ConnectionChoice[] | null;
}) {
  const { opening, open, close } = useOpening();
  return (
    <>
      <PageAction primary="New alert" onClick={(e) => open(e.currentTarget)}>
        <BellPlus className="size-4" />
        New alert
      </PageAction>
      <AlertPanelHost
        opening={opening}
        onClose={close}
        projectId={projectId}
        ruleId={null}
        initial={NEW_ALERT}
        connections={connections}
      />
    </>
  );
}

function AlertPanel({
  projectId,
  ruleId,
  initial,
  connections,
  onClose,
  returnFocus,
}: PanelProps & { onClose: () => void; returnFocus: () => HTMLElement | null }) {
  const titleId = useId();
  const id = useId();
  const panel = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const [draft, setDraft] = useState<AlertDraft>(initial);
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();
  const said = useRef<HTMLParagraphElement>(null);
  // An alert's panel goes with its card.
  const { gone } = useLanding(ALERTS_HEADING);

  // The form runs past the panel's height: what is wrong is brought into
  // view, below the field the person may have scrolled away from.
  useEffect(() => {
    if (error) said.current?.scrollIntoView?.({ block: "nearest" });
  }, [error]);

  const change = (next: AlertDraft) => {
    setDraft(next);
    setError(null);
  };
  const unit = unitOf(draft.measure, draft.aggregation);
  const suffix = UNIT_SUFFIX[unit];

  function submit() {
    const checked = inputOf(draft);
    if (!checked.ok) {
      setError(checked.error);
      return;
    }
    setError(null);
    start(async () => {
      try {
        const r = await saveAlertAction(projectId, ruleId, checked.input);
        if (gone(r)) return;
        if (!r.ok) {
          setError(r.error);
          return;
        }
        // In the transition, with the page the action drew again, so the
        // panel shuts as the alert is listed rather than a frame before.
        startTransition(onClose);
      } catch (err) {
        unstable_rethrow(err);
        setError("Network error. Try again.");
      }
    });
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
          if (!pending) submit();
        }}
      >
        <SidePanelHeader
          icon={BellRing}
          titleId={titleId}
          title={ruleId ? `Edit ${initial.name}` : "New alert"}
          onClose={onClose}
        >
          <p className="text-sm text-muted-foreground">
            A measure of this project&rsquo;s spans over a window against an alert threshold and
            an optional warning one, evaluated every minute. Its incidents reach the project&rsquo;s
            feed and the connectors it links.
          </p>
        </SidePanelHeader>
        <div className="min-h-0 flex-1 overflow-y-auto px-3.5">
          <SidePanelRow label="Name" htmlFor={`${id}-name`}>
            <Input
              ref={field}
              id={`${id}-name`}
              value={draft.name}
              onChange={(e) => change({ ...draft, name: e.target.value })}
              placeholder="e.g. Slow model calls"
              autoComplete="off"
              disabled={pending}
              className="max-w-md"
            />
          </SidePanelRow>
          <SidePanelRow label="Reads" htmlFor={`${id}-kind`}>
            <Select
              value={draft.kind}
              onValueChange={(kind) => change(withKind(draft, kind as AlertKind))}
              disabled={pending}
            >
              <SelectTrigger id={`${id}-kind`} className="w-full max-w-md">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {ALERT_KINDS.map((kind) => (
                  <SelectItem key={kind} value={kind}>
                    {KIND_PLURAL[kind].replace(/^./, (c) => c.toUpperCase())}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </SidePanelRow>
          <SidePanelRow label="Measure" htmlFor={`${id}-measure`}>
            <Select
              value={draft.measure}
              onValueChange={(measure) => change(withMeasure(draft, measure as RuleMeasure))}
              disabled={pending}
            >
              <SelectTrigger id={`${id}-measure`} className="w-full max-w-md">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {measuresFor(draft.kind).map((measure) => (
                  <SelectItem key={measure} value={measure}>
                    {MEASURE_LABEL[measure]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </SidePanelRow>
          <SidePanelRow label="Folded as" htmlFor={`${id}-aggregation`}>
            <Select
              value={draft.aggregation}
              onValueChange={(aggregation) =>
                change({ ...draft, aggregation: aggregation as RuleAggregation })
              }
              disabled={pending}
            >
              <SelectTrigger id={`${id}-aggregation`} className="w-full max-w-md">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {aggregationsFor(draft.measure).map((aggregation) => (
                  <SelectItem key={aggregation} value={aggregation}>
                    {aggregationLabel(draft.measure, aggregation)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </SidePanelRow>
          <SidePanelRow label="Window" htmlFor={`${id}-window`}>
            <Select
              value={draft.window}
              onValueChange={(window) => change({ ...draft, window: window as RuleWindow })}
              disabled={pending}
            >
              <SelectTrigger id={`${id}-window`} className="w-full max-w-md">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {Object.values(RuleWindow).map((window) => (
                  <SelectItem key={window} value={window}>
                    {WINDOW_LABEL[window]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </SidePanelRow>
          <SidePanelRow label="Filters">
            <div className="grid max-w-md gap-2">
              {filterFieldsFor(draft.kind, draft.measure).map((name) => (
                <div key={name} className="grid gap-1 sm:grid-cols-[7rem_minmax(0,1fr)] sm:items-center sm:gap-3">
                  <Label htmlFor={`${id}-filter-${name}`} className="font-normal text-muted-foreground">
                    {FILTER_LABEL[name]}
                  </Label>
                  <Input
                    id={`${id}-filter-${name}`}
                    value={draft.filters[name]}
                    onChange={(e) =>
                      change({ ...draft, filters: { ...draft.filters, [name]: e.target.value } })
                    }
                    placeholder={`Any ${FILTER_LABEL[name].toLowerCase()}`}
                    autoComplete="off"
                    spellCheck={false}
                    disabled={pending}
                    aria-describedby={`${id}-filters-hint`}
                  />
                </div>
              ))}
              {filtersStatus(draft.measure) && (
                <fieldset className="grid gap-1 sm:grid-cols-[7rem_minmax(0,1fr)] sm:items-center sm:gap-3">
                  <legend className="sr-only">Status</legend>
                  <span aria-hidden="true" className="text-sm text-muted-foreground">
                    Status
                  </span>
                  <div className="flex min-h-9 flex-wrap items-center gap-x-3.5 gap-y-1.5">
                    {FILTER_STATUSES.map((status) => (
                      <Label key={status} className="font-normal">
                        <Checkbox
                          checked={draft.statuses.includes(status)}
                          onCheckedChange={(on) =>
                            change({
                              ...draft,
                              statuses:
                                on === true
                                  ? [...draft.statuses, status]
                                  : draft.statuses.filter((s: SpanStatus) => s !== status),
                            })
                          }
                          disabled={pending}
                        />
                        {STATUS_LABEL[status]}
                      </Label>
                    ))}
                  </div>
                </fieldset>
              )}
              <p id={`${id}-filters-hint`} className="text-muted-foreground">
                Separate values with commas, a value that holds one between double quotes; an
                empty field admits every value, and no status ticked admits every status.
              </p>
            </div>
          </SidePanelRow>
          <SidePanelRow label="Compared as" htmlFor={`${id}-operator`}>
            <Select
              value={draft.operator}
              onValueChange={(operator) => change({ ...draft, operator: operator as RuleOperator })}
              disabled={pending}
            >
              <SelectTrigger id={`${id}-operator`} className="w-full max-w-md">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {Object.values(RuleOperator).map((operator) => (
                  <SelectItem key={operator} value={operator}>
                    {OPERATOR_LABEL[operator]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </SidePanelRow>
          <SidePanelRow label="Alert when" htmlFor={`${id}-alert`}>
            <div className="flex max-w-md flex-wrap items-center gap-3">
              <span className="text-muted-foreground">{OPERATOR_LABEL[draft.operator]}</span>
              <Threshold
                id={`${id}-alert`}
                value={draft.alertThreshold}
                suffix={suffix}
                onChange={(alertThreshold) => change({ ...draft, alertThreshold })}
                disabled={pending}
              />
            </div>
          </SidePanelRow>
          {takesWarning(draft.operator) && (
            <SidePanelRow label="Warn when" htmlFor={`${id}-warning`}>
              <div className="flex max-w-md flex-wrap items-center gap-3">
                <span className="text-muted-foreground">{OPERATOR_LABEL[draft.operator]}</span>
                <Threshold
                  id={`${id}-warning`}
                  value={draft.warningThreshold}
                  suffix={suffix}
                  onChange={(warningThreshold) => change({ ...draft, warningThreshold })}
                  disabled={pending}
                  placeholder="Optional"
                  hint={`${id}-warning-hint`}
                />
              </div>
              <p id={`${id}-warning-hint`} className="text-muted-foreground">
                Met before the alert threshold; empty for an alert alone.
              </p>
            </SidePanelRow>
          )}
          <SidePanelRow label="No data" htmlFor={`${id}-missing`}>
            <Select
              value={draft.missingData}
              onValueChange={(missingData) => change({ ...draft, missingData: missingData as MissingData })}
              disabled={pending}
            >
              <SelectTrigger
                id={`${id}-missing`}
                aria-describedby={`${id}-missing-hint`}
                className="w-full max-w-md"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {Object.values(MissingData).map((missing) => (
                  <SelectItem key={missing} value={missing}>
                    {MISSING_DATA_LABEL[missing]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p id={`${id}-missing-hint`} className="text-muted-foreground">
              What a window holding no span means.
            </p>
          </SidePanelRow>
          <SidePanelRow label="Notify again" htmlFor={`${id}-renotify`}>
            <div className="flex max-w-md items-center gap-3">
              <span className="text-muted-foreground">Every</span>
              <Input
                id={`${id}-renotify`}
                inputMode="numeric"
                value={draft.renotifyMinutes}
                onChange={(e) => change({ ...draft, renotifyMinutes: e.target.value })}
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
        </div>
        <SidePanelFooter>
          <Button type="submit" disabled={pending}>
            {pending ? "Saving…" : ruleId ? "Save alert" : "Make alert"}
          </Button>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
        </SidePanelFooter>
      </form>
    </SidePanel>
  );
}

/** A threshold, typed in the alert's unit. */
function Threshold({
  id,
  value,
  suffix,
  onChange,
  disabled,
  placeholder,
  hint,
}: {
  id: string;
  value: string;
  suffix: string;
  onChange: (value: string) => void;
  disabled: boolean;
  placeholder?: string;
  /** The id of the hint below the field. */
  hint?: string;
}) {
  const described = [suffix ? `${id}-unit` : null, hint].filter(Boolean).join(" ");
  return (
    <span className="flex items-center gap-2">
      <Input
        id={id}
        inputMode="decimal"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        autoComplete="off"
        disabled={disabled}
        aria-describedby={described || undefined}
        className="w-36"
      />
      {suffix && (
        <span id={`${id}-unit`} className="text-muted-foreground">
          {suffix}
        </span>
      )}
    </span>
  );
}

/** The connectors an alert's notices go to: each of the project's, switched
 *  on or off, at its level and in its hours; one the alert links that the
 *  project no longer has, listed so it is not dropped unseen. */
function Links({
  id,
  links,
  connections,
  onChange,
  disabled,
}: {
  id: string;
  links: LinkDraft[];
  connections: ConnectionChoice[] | null;
  onChange: (links: LinkDraft[]) => void;
  disabled: boolean;
}) {
  if (connections === null) {
    return (
      <p className="text-muted-foreground">
        The project&rsquo;s connectors could not be read just now; the alert keeps those it links.
      </p>
    );
  }
  const known = new Set(connections.map((c) => c.id));
  const rows: ConnectionChoice[] = [
    ...connections,
    ...links
      .filter((link) => !known.has(link.connectionId))
      .map((link) => ({ id: link.connectionId, name: "A removed connector", status: "revoked" as const, eventKinds: null })),
  ];
  if (rows.length === 0) {
    return (
      <p className="text-muted-foreground">
        No connectors yet: the alert&rsquo;s incidents reach the project&rsquo;s feed alone. Connect
        Slack or a webhook on the Connectors page.
      </p>
    );
  }
  const set = (connectionId: string, next: LinkDraft | null) =>
    onChange(
      next === null
        ? links.filter((link) => link.connectionId !== connectionId)
        : links.some((link) => link.connectionId === connectionId)
          ? links.map((link) => (link.connectionId === connectionId ? next : link))
          : [...links, next],
    );
  const full = links.length >= LINKS_MAX;
  const levelHint = `${id}-level-hint`;
  return (
    <>
      <ul className="grid max-w-xl gap-3" aria-label="Connectors">
        {rows.map((connection) => {
          const link = links.find((l) => l.connectionId === connection.id) ?? null;
          const switchId = `${id}-link-${connection.id}`;
          const status = CONNECTION_STATUS_LABEL[connection.status];
          const taken = incidentKindsLabel(connection.eventKinds);
          return (
            <li key={connection.id} className="grid gap-3 rounded-menu border border-border p-3">
              <div className="flex items-center justify-between gap-3">
                <span className="flex min-w-0 items-center gap-2">
                  <Label id={`${switchId}-name`} htmlFor={switchId} className="min-w-0 truncate font-normal">
                    {connection.name}
                  </Label>
                  {status && (
                    <Badge variant="secondary" className="shrink-0 font-normal">
                      {status}
                    </Badge>
                  )}
                  {taken && (
                    <Badge variant="secondary" className="shrink-0 font-normal">
                      {taken}
                    </Badge>
                  )}
                </span>
                <Switch
                  id={switchId}
                  checked={link !== null}
                  onCheckedChange={(on) =>
                    set(connection.id, on ? { connectionId: connection.id, level: RuleLevel.Warning, hours: null } : null)
                  }
                  disabled={disabled || (link === null && full)}
                />
              </div>
              {link && (
                <LinkFields
                  id={switchId}
                  link={link}
                  levelHint={levelHint}
                  onChange={(next) => set(connection.id, next)}
                  disabled={disabled}
                />
              )}
            </li>
          );
        })}
      </ul>
      {links.length > 0 && (
        <p id={levelHint} className="text-muted-foreground">
          A no-data incident reaches only the connectors that hear warnings and alerts.
        </p>
      )}
    </>
  );
}

/** A linked connector's level and hours. */
function LinkFields({
  id,
  link,
  levelHint,
  onChange,
  disabled,
}: {
  id: string;
  link: LinkDraft;
  /** The id of the line under the connectors on what each level hears. */
  levelHint: string;
  onChange: (link: LinkDraft) => void;
  disabled: boolean;
}) {
  const hours = link.hours;
  return (
    // Named by its connector, so a control here says whose it is.
    <div role="group" aria-labelledby={`${id}-name`} className="grid gap-3">
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="grid gap-1.5">
          <Label htmlFor={`${id}-level`} className="font-normal text-muted-foreground">
            Hears
          </Label>
          <Select
            value={link.level}
            onValueChange={(level) => onChange({ ...link, level: level as RuleLevel })}
            disabled={disabled}
          >
            <SelectTrigger id={`${id}-level`} aria-describedby={levelHint} className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {Object.values(RuleLevel).map((level) => (
                <SelectItem key={level} value={level}>
                  {LEVEL_LABEL[level]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-1.5">
          <Label htmlFor={`${id}-when`} className="font-normal text-muted-foreground">
            When
          </Label>
          <Select
            value={hours ? "set" : "every"}
            onValueChange={(mode) =>
              onChange({
                ...link,
                hours:
                  mode === "set"
                    ? { days: [1, 2, 3, 4, 5], from: "09:00", to: "18:00", utc_offset: browserOffset() }
                    : null,
              })
            }
            disabled={disabled}
          >
            <SelectTrigger id={`${id}-when`} className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="every">At any hour</SelectItem>
              <SelectItem value="set">Set hours</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>
      {hours && (
        <div className="grid gap-3">
          <ToggleGroup
            type="multiple"
            variant="outline"
            size="sm"
            value={hours.days.map(String)}
            onValueChange={(days) => onChange({ ...link, hours: { ...hours, days: days.map(Number) } })}
            aria-label="Days"
            disabled={disabled}
            className="flex-wrap"
          >
            {DAYS.map(({ day, label }) => (
              <ToggleGroupItem key={day} value={String(day)}>
                {label}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
          {/* Each label with its field, so a narrow panel never leaves one at a line's end. */}
          <div className="flex flex-wrap items-center gap-3">
            <span className="flex items-center gap-3">
              <Label htmlFor={`${id}-from`} className="font-normal text-muted-foreground">
                From
              </Label>
              <Input
                id={`${id}-from`}
                type="time"
                value={hours.from}
                onChange={(e) => onChange({ ...link, hours: { ...hours, from: e.target.value } })}
                disabled={disabled}
                aria-describedby={`${id}-hours-hint`}
                className="w-32"
              />
            </span>
            <span className="flex items-center gap-3">
              <Label htmlFor={`${id}-to`} className="font-normal text-muted-foreground">
                to
              </Label>
              <Input
                id={`${id}-to`}
                type="time"
                value={hours.to}
                onChange={(e) => onChange({ ...link, hours: { ...hours, to: e.target.value } })}
                disabled={disabled}
                aria-describedby={`${id}-hours-hint`}
                className="w-32"
              />
            </span>
            <span className="flex items-center gap-3">
              <Label htmlFor={`${id}-offset`} className="font-normal text-muted-foreground">
                Offset
              </Label>
              <Select
                value={String(hours.utc_offset)}
                onValueChange={(offset) => onChange({ ...link, hours: { ...hours, utc_offset: Number(offset) } })}
                disabled={disabled}
              >
                <SelectTrigger id={`${id}-offset`} className="w-36">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {/* The quarter-hours, and an offset the lane kept that is none of them. */}
                  {[...new Set([...OFFSETS, hours.utc_offset])]
                    .sort((a, b) => a - b)
                    .map((offset) => (
                      <SelectItem key={offset} value={String(offset)}>
                        {offsetWords(offset)}
                      </SelectItem>
                    ))}
                </SelectContent>
              </Select>
            </span>
          </div>
          <p id={`${id}-hours-hint`} className="text-xs text-muted-foreground">
            A time before the start runs past midnight; the same time both ends is the whole day.
          </p>
        </div>
      )}
    </div>
  );
}
