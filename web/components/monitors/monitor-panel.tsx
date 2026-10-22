"use client";

import { startTransition, useEffect, useId, useMemo, useRef, useState, useTransition } from "react";
import Link from "next/link";
import { unstable_rethrow } from "next/navigation";
import { CheckIcon, ChevronsUpDownIcon, Radar } from "lucide-react";

import { LocalTime } from "@/components/local-time";
import { ReportLines } from "@/components/monitors/report-lines";
import {
  SidePanel,
  SidePanelFooter,
  SidePanelHeader,
  SidePanelRow,
} from "@/components/side-panel";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import {
  blankForm,
  checkForm,
  formOf,
  monitorHref,
  slugFromName,
  type MonitorForm,
  type Timing,
  type Typed,
  type Unit,
} from "@/lib/monitors";
import type { Monitor } from "@/lib/server/entities/monitors";
import { cn } from "@/lib/utils";

import {
  createMonitorAction,
  previewScheduleAction,
  updateMonitorAction,
} from "@/app/(app)/[organization]/[project]/monitors/actions";

/** The zone this browser keeps, when it names one. */
function browserZone(): string | null {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || null;
  } catch {
    return null;
  }
}

const UNIT_LABEL: Record<Unit, string> = { seconds: "seconds", minutes: "minutes", hours: "hours" };

/** What the schedule's preview shows: nothing yet, the next due times, or
 *  why the lane could not read the timing. */
type Preview = { kind: "idle" } | { kind: "ok"; next: string[] } | { kind: "error"; error: string };

/** The timing as the preview lane takes it, or `null` where nothing is
 *  expected or the timing is not yet a number. */
function previewOf(form: MonitorForm): { schedule?: string; interval_s?: number; timezone?: string } | null {
  if (form.timing === "none") return null;
  if (form.timing === "cron") {
    const schedule = form.cron.trim();
    return schedule ? { schedule, timezone: form.timezone } : null;
  }
  const value = Number(form.every.value.trim());
  const seconds = value * (form.every.unit === "hours" ? 3_600 : form.every.unit === "minutes" ? 60 : 1);
  return Number.isInteger(seconds) && seconds > 0 ? { interval_s: seconds } : null;
}

// A monitor made or changed in the console's side panel, full over the
// content area as Mint key and New project open: its name and slug, the
// agent its runs carry, when they are expected and in which zone, its grace
// and expected length, and its bounds; the next three due times shown as the
// schedule is typed. Made, the panel says how its runs are reported. The
// lane checks every field again and names the one it cannot take.
export function MonitorPanel({
  organization,
  slug: projectSlug,
  projectId,
  zones,
  monitor,
  onClose,
  returnFocus,
}: {
  organization: string;
  slug: string;
  projectId: string;
  /** Every zone a monitor may be in; `null` where the list could not be read. */
  zones: string[] | null;
  /** The monitor being changed; none for a new one. */
  monitor?: Monitor;
  onClose: () => void;
  returnFocus?: () => HTMLElement | null;
}) {
  const titleId = useId();
  const field = useId();
  const panel = useRef<HTMLDivElement>(null);
  const first = useRef<HTMLInputElement>(null);
  const [own] = useState(browserZone);
  const [form, setForm] = useState<MonitorForm>(() => {
    if (monitor) return formOf(monitor);
    return blankForm(own && zones?.includes(own) ? own : "UTC");
  });
  const [slugTyped, setSlugTyped] = useState(Boolean(monitor));
  const [error, setError] = useState<{ field?: keyof MonitorForm; message: string } | null>(null);
  const [made, setMade] = useState<Monitor | null>(null);
  const [preview, setPreview] = useState<Preview>({ kind: "idle" });
  const [pending, start] = useTransition();

  const slug = slugTyped ? form.slug : slugFromName(form.name);
  const timing = previewOf(form);
  const timingKey = JSON.stringify(timing);

  // The next due times, asked for a beat after the typing stops; an answer
  // to an older timing is dropped.
  useEffect(() => {
    if (timingKey === "null") return;
    let live = true;
    const timer = setTimeout(async () => {
      const answer = await previewScheduleAction(projectId, JSON.parse(timingKey));
      if (!live) return;
      setPreview(
        answer.status === "ok" ? { kind: "ok", next: answer.next } : { kind: "error", error: answer.error },
      );
    }, 400);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [projectId, timingKey]);

  function set<K extends keyof MonitorForm>(key: K, value: MonitorForm[K]) {
    setForm((was) => ({ ...was, [key]: value }));
    setError(null);
  }

  function submit() {
    const checked = checkForm({ ...form, slug });
    if (!checked.ok) {
      setError({ field: checked.field, message: checked.message });
      return;
    }
    setError(null);
    start(async () => {
      try {
        const answer = monitor
          ? await updateMonitorAction(projectId, monitor.slug, checked.body)
          : await createMonitorAction(projectId, checked.body);
        if (answer.status === "error") {
          setError({ message: answer.error });
          return;
        }
        // In the transition, with the list the action drew again.
        startTransition(() => (monitor ? onClose() : setMade(answer.monitor)));
      } catch (err) {
        unstable_rethrow(err);
        setError({ message: "Network error. Try again." });
      }
    });
  }

  const invalid = (key: keyof MonitorForm) =>
    error?.field === key ? { "aria-invalid": true, "aria-describedby": `${field}-error` } : {};

  if (made) {
    return (
      <SidePanel panel={panel} titleId={titleId} onClose={onClose} full returnFocus={returnFocus}>
        <SidePanelHeader icon={Radar} titleId={titleId} title={`${made.name} is watching`} onClose={onClose}>
          <p className="text-sm text-muted-foreground">
            Report its runs one of these ways, and each lands on its monitor within seconds.
          </p>
        </SidePanelHeader>
        <div className="min-h-0 flex-1 overflow-y-auto px-3.5 py-3.5">
          <div className="max-w-2xl">
            <ReportLines slug={made.slug} agent={made.agent} />
          </div>
        </div>
        <SidePanelFooter>
          <Button asChild variant="outline">
            <Link href={monitorHref(organization, projectSlug, made.slug)} onClick={onClose}>
              Open the monitor
            </Link>
          </Button>
          <Button type="button" onClick={onClose}>
            Done
          </Button>
        </SidePanelFooter>
      </SidePanel>
    );
  }

  return (
    <SidePanel
      panel={panel}
      titleId={titleId}
      onClose={onClose}
      full
      initialFocus={first}
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
          icon={Radar}
          titleId={titleId}
          title={monitor ? `Edit ${monitor.name}` : "New monitor"}
          onClose={onClose}
        >
          <p className="text-sm text-muted-foreground">
            An agent&apos;s contract: when its runs are expected, how long one may go quiet and
            how many steps it may take.
          </p>
        </SidePanelHeader>
        <div className="min-h-0 flex-1 overflow-y-auto px-3.5 py-2">
          <SidePanelRow label="Name" htmlFor={`${field}-name`}>
            <Input
              ref={first}
              id={`${field}-name`}
              value={form.name}
              onChange={(e) => set("name", e.target.value)}
              maxLength={60}
              placeholder="e.g. Nightly report"
              autoComplete="off"
              required
              disabled={pending}
              className="max-w-md"
              {...invalid("name")}
            />
          </SidePanelRow>
          <SidePanelRow label="Slug" htmlFor={`${field}-slug`}>
            {monitor ? (
              <p className="flex h-9 items-center font-mono">{monitor.slug}</p>
            ) : (
              <Input
                id={`${field}-slug`}
                value={slug}
                onChange={(e) => {
                  setSlugTyped(true);
                  set("slug", e.target.value.toLowerCase());
                }}
                maxLength={48}
                autoComplete="off"
                spellCheck={false}
                disabled={pending}
                className="max-w-md font-mono"
                {...invalid("slug")}
              />
            )}
            <p className="text-muted-foreground">
              {monitor
                ? "Fixed: every crontab line that pings it carries it."
                : "Made from the name. Every ping names it, so it is fixed once saved."}
            </p>
          </SidePanelRow>
          <SidePanelRow label="Agent" htmlFor={`${field}-agent`}>
            <Input
              id={`${field}-agent`}
              value={form.agent}
              onChange={(e) => set("agent", e.target.value)}
              maxLength={128}
              placeholder={slug || "the slug"}
              autoComplete="off"
              spellCheck={false}
              disabled={pending}
              className="max-w-md font-mono"
              {...invalid("agent")}
            />
            <p className="text-muted-foreground">
              The <code>telmoni.agent</code> its runs carry: the slug unless you name another. One
              monitor an agent.
            </p>
          </SidePanelRow>
          <SidePanelRow label="Schedule">
            <ToggleGroup
              type="single"
              variant="outline"
              value={form.timing}
              onValueChange={(next) => next && set("timing", next as Timing)}
              disabled={pending}
              aria-label="When its runs are expected"
            >
              <ToggleGroupItem value="cron">Cron line</ToggleGroupItem>
              <ToggleGroupItem value="interval">Every</ToggleGroupItem>
              <ToggleGroupItem value="none">None</ToggleGroupItem>
            </ToggleGroup>
            {form.timing === "cron" && (
              <div className="grid max-w-md gap-1.5">
                <Input
                  id={`${field}-cron`}
                  aria-label="Cron line"
                  value={form.cron}
                  onChange={(e) => set("cron", e.target.value)}
                  maxLength={200}
                  placeholder="0 2 * * *"
                  autoComplete="off"
                  spellCheck={false}
                  disabled={pending}
                  className="font-mono"
                  {...invalid("cron")}
                />
                {zones ? (
                  <ZonePicker
                    zones={zones}
                    own={own}
                    value={form.timezone}
                    onChange={(zone) => set("timezone", zone)}
                    disabled={pending}
                  />
                ) : (
                  <Input
                    aria-label="Timezone"
                    value={form.timezone}
                    onChange={(e) => set("timezone", e.target.value)}
                    maxLength={64}
                    autoComplete="off"
                    spellCheck={false}
                    disabled={pending}
                  />
                )}
                <p className="text-muted-foreground">
                  Five fields, as a crontab reads them, in this zone.
                </p>
              </div>
            )}
            {form.timing === "interval" && (
              <DurationInput
                label="How often it runs"
                value={form.every}
                onChange={(every) => set("every", every)}
                units={["minutes", "hours"]}
                disabled={pending}
                invalid={invalid("every")}
              />
            )}
            {form.timing === "none" ? (
              <p className="text-muted-foreground">
                No run is expected, so it is never late; its runs are still held to its bounds.
              </p>
            ) : timing !== null ? (
              <NextDue preview={preview} />
            ) : null}
          </SidePanelRow>
          {form.timing !== "none" && (
            <SidePanelRow label="Grace period">
              <DurationInput
                label="Grace period"
                value={form.grace}
                onChange={(grace) => set("grace", grace)}
                units={["seconds", "minutes", "hours"]}
                disabled={pending}
                invalid={invalid("grace")}
              />
              <p className="text-muted-foreground">
                How long past its time a run may start before the monitor is late.
              </p>
            </SidePanelRow>
          )}
          <SidePanelRow label="Expected duration">
            <DurationInput
              label="Expected duration"
              value={form.expected}
              onChange={(expected) => set("expected", expected)}
              units={["seconds", "minutes", "hours"]}
              disabled={pending}
              invalid={invalid("expected")}
            />
            <p className="text-muted-foreground">
              Optional. A run still open four times this long, or a day where none is set, is
              ended as failed.
            </p>
          </SidePanelRow>
          <SidePanelRow label="Stall timeout">
            <DurationInput
              label="Stall timeout"
              value={form.stall}
              onChange={(stall) => set("stall", stall)}
              units={["seconds", "minutes", "hours"]}
              disabled={pending}
              invalid={invalid("stall")}
            />
            <p className="text-muted-foreground">
              Optional. How long a run may go quiet before it is stalled.
            </p>
          </SidePanelRow>
          <SidePanelRow label="Step cap" htmlFor={`${field}-steps`}>
            <Input
              id={`${field}-steps`}
              value={form.steps}
              onChange={(e) => set("steps", e.target.value)}
              inputMode="numeric"
              placeholder="No cap"
              autoComplete="off"
              disabled={pending}
              className="max-w-40"
              {...invalid("steps")}
            />
            <p className="text-muted-foreground">
              Optional. How many steps a run may take. A run&apos;s own code may set tighter
              bounds, and the tighter holds.
            </p>
          </SidePanelRow>
          {error && (
            <p id={`${field}-error`} className="py-2 text-sm text-destructive" role="alert">
              {error.message}
            </p>
          )}
        </div>
        <SidePanelFooter>
          <Button type="submit" disabled={pending}>
            {pending ? "Saving…" : monitor ? "Save" : "Create monitor"}
          </Button>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
        </SidePanelFooter>
      </form>
    </SidePanel>
  );
}

/** A length typed as a number and its unit. */
function DurationInput({
  label,
  value,
  onChange,
  units,
  disabled,
  invalid,
}: {
  label: string;
  value: Typed;
  onChange: (next: Typed) => void;
  units: readonly Unit[];
  disabled: boolean;
  invalid: object;
}) {
  return (
    <div className="flex max-w-md items-center gap-2">
      <Input
        aria-label={label}
        value={value.value}
        onChange={(e) => onChange({ ...value, value: e.target.value })}
        inputMode="decimal"
        autoComplete="off"
        disabled={disabled}
        className="max-w-28"
        {...invalid}
      />
      <Select
        value={value.unit}
        onValueChange={(unit) => onChange({ ...value, unit: unit as Unit })}
        disabled={disabled}
      >
        <SelectTrigger aria-label={`${label}, in`} className="w-32">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {units.map((unit) => (
            <SelectItem key={unit} value={unit}>
              {UNIT_LABEL[unit]}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}

/** The next three times a run would be due, as the lane reads the timing. */
function NextDue({ preview }: { preview: Preview }) {
  if (preview.kind === "idle") return null;
  if (preview.kind === "error") {
    return <p className="text-destructive">{preview.error}</p>;
  }
  if (preview.next.length === 0) return <p className="text-muted-foreground">Never due.</p>;
  return (
    <p className="text-muted-foreground">
      Next due{" "}
      {preview.next.map((iso, i) => (
        <span key={iso}>
          {i > 0 ? (i === preview.next.length - 1 ? " and " : ", ") : null}
          <LocalTime iso={iso} mode="datetime" />
        </span>
      ))}
      .
    </p>
  );
}

/** The zone a cron line is read in: a combobox of every zone the server
 *  knows, this browser's own first. */
function ZonePicker({
  zones,
  own,
  value,
  onChange,
  disabled,
}: {
  zones: string[];
  own: string | null;
  value: string;
  onChange: (zone: string) => void;
  disabled: boolean;
}) {
  const [open, setOpen] = useState(false);
  const ordered = useMemo(
    () => (own && zones.includes(own) ? [own, ...zones.filter((z) => z !== own)] : zones),
    [zones, own],
  );
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="outline"
          role="combobox"
          aria-expanded={open}
          aria-label={`Timezone, ${value}`}
          disabled={disabled}
          className="w-full justify-between font-normal"
        >
          <span className="truncate">{value}</span>
          <ChevronsUpDownIcon className="text-muted-foreground" aria-hidden="true" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-(--radix-popover-trigger-width) p-0">
        <Command>
          <CommandInput placeholder="Search zones" />
          <CommandList>
            <CommandEmpty>No zone by that name.</CommandEmpty>
            {ordered.map((zone) => (
              <CommandItem
                key={zone}
                value={zone}
                onSelect={() => {
                  onChange(zone);
                  setOpen(false);
                }}
              >
                <CheckIcon
                  className={cn("size-4", zone === value ? "opacity-100" : "opacity-0")}
                  aria-hidden="true"
                />
                <span className="truncate">{zone}</span>
                {zone === own && (
                  <span className="ml-auto text-xs text-muted-foreground">This browser&apos;s</span>
                )}
              </CommandItem>
            ))}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
