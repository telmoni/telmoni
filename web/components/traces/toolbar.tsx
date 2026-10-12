"use client";

import { Fragment, useEffect, useState, useTransition, type ReactNode } from "react";
import {
  ChartColumnIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  CircleXIcon,
  CoinsIcon,
  Columns3Icon,
  DownloadIcon,
  InfoIcon,
  RefreshCwIcon,
  Rows3Icon,
  TableIcon,
  TimerIcon,
  type LucideIcon,
} from "lucide-react";
import { useRouter } from "next/navigation";

import { ViewsMenu } from "@/components/traces/views-menu";
import { useListNavigation } from "@/components/traces/navigate";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Toggle } from "@/components/ui/toggle";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { WindowPicker } from "@/components/window-picker";
import type { TraceQuery, TraceView } from "@/lib/server/entities/traces";
import {
  DENSITIES,
  REFRESH_INTERVALS,
  orderedColumns,
  refreshLabel,
  type Density,
  type RefreshSeconds,
} from "@/lib/trace-prefs";
import {
  COLUMN_MENU,
  DEFAULT_COLUMNS,
  DOWNLOAD_MAX,
  TRACE_COLUMNS,
  TRACE_ID_COLUMN,
  TRACE_PRESETS,
  downloadHref,
  formatCount,
  presetActive,
  presetToggled,
  tracesHref,
  type ColumnGroup,
  type Preset,
  type TraceUi,
} from "@/lib/traces";
import type { TraceColumn } from "@/lib/types/enums";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";
import { cn } from "@/lib/utils";

type Props = {
  organization: string;
  slug: string;
  projectId: string;
  query: TraceQuery;
  ui: TraceUi;
  /** How many traces the filters admit. */
  total: number;
  views: TraceView[] | null;
  canSave: boolean;
};

/** Each ready-made query's icon, as Langfuse marks Quality, Slow and Cost. */
const PRESET_ICON: Record<Preset["id"], LucideIcon> = {
  errors: CircleXIcon,
  slow: TimerIcon,
  tokens: CoinsIcon,
};

// The trace list's toolbar, Langfuse's own set on one row: the ready-made
// queries, the saved views and the table or the charts on the left; the
// columns, the rows' density and a download on the right. The window and the
// refresh stand on the title's row, as Langfuse keeps them at the top of the
// page (`TraceHeaderControls`), and the filters' panel opens and closes from
// its own heading. Every control is a shadcn one; each that changes what the
// list holds changes its URL, and each that changes only how it looks is kept
// in the browser (`lib/trace-prefs.ts`).
export function TraceToolbar(props: Props) {
  const { organization, slug, query, ui } = props;
  const { go, pending } = useListNavigation();
  const href = (next: TraceQuery, nextUi: TraceUi = ui) =>
    tracesHref(organization, slug, next, {}, nextUi);

  return (
    <div
      className="flex flex-wrap items-center gap-2"
      aria-busy={pending}
      data-pending={pending ? "" : undefined}
    >
      {TRACE_PRESETS.map((preset) => {
        const Icon = PRESET_ICON[preset.id];
        return (
          <Tooltip key={preset.id}>
            <TooltipTrigger asChild>
              <Toggle
                variant="outline"
                pressed={presetActive(query, preset.id)}
                onPressedChange={() => go(href(presetToggled(query, preset.id)))}
              >
                <Icon />
                {preset.label}
              </Toggle>
            </TooltipTrigger>
            <TooltipContent>{preset.hint}</TooltipContent>
          </Tooltip>
        );
      })}

      <ViewsMenu
        organization={organization}
        slug={slug}
        projectId={props.projectId}
        query={query}
        ui={ui}
        views={props.views}
        canSave={props.canSave}
      />

      <Tabs
        value={ui.display ?? "table"}
        onValueChange={(display) =>
          go(href(query, { ...ui, display: display === "chart" ? "chart" : undefined }))
        }
      >
        <TabsList>
          <TabsTrigger value="table">
            <TableIcon />
            Table
          </TabsTrigger>
          <TabsTrigger value="chart">
            <ChartColumnIcon />
            Chart
          </TabsTrigger>
        </TabsList>
      </Tabs>

      <div className="ml-auto flex items-center gap-2">
        <ColumnsMenu />
        <DensityMenu />
        <DownloadMenu organization={organization} slug={slug} query={query} total={props.total} />
      </div>
    </div>
  );
}

// The window the list reads and its refresh, on the title's row as Langfuse
// keeps its "Past 1 day" and refresh at the top of the page.
export function TraceHeaderControls({ days }: { days: number }) {
  return (
    <div className="flex items-center gap-2">
      <WindowPicker days={days} />
      <RefreshControl />
    </div>
  );
}

/** A refresh now, and how often the page reads itself again: only while it
 *  is in view, so a tab left open asks nothing of the server. */
function RefreshControl() {
  const router = useRouter();
  const { refresh } = useTracePrefs();
  const [refreshing, start] = useTransition();

  useEffect(() => {
    if (refresh === 0) return;
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") start(() => router.refresh());
    }, refresh * 1_000);
    return () => window.clearInterval(timer);
  }, [refresh, router]);

  return (
    <div className="flex items-center">
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="outline"
            size="icon"
            className="rounded-r-none"
            aria-label="Refresh"
            onClick={() => start(() => router.refresh())}
          >
            <RefreshCwIcon className={cn(refreshing && "animate-spin")} />
          </Button>
        </TooltipTrigger>
        <TooltipContent>Refresh</TooltipContent>
      </Tooltip>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" className="-ml-px rounded-l-none" aria-label="Refresh every">
            {refreshLabel(refresh)}
            <ChevronDownIcon className="opacity-50" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuLabel>Refresh every</DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={String(refresh)}
            onValueChange={(value) => setTracePrefs({ refresh: Number(value) as RefreshSeconds })}
          >
            {REFRESH_INTERVALS.map((seconds) => (
              <DropdownMenuRadioItem key={seconds} value={String(seconds)}>
                {seconds === 0 ? "Never" : refreshLabel(seconds)}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

/** Which columns the table shows, as Langfuse's "Columns" picks them: the
 *  trace's id ticked for good, the counts and the tokens each a group that
 *  opens under its heading with how many of it show, what a column means
 *  behind its ⓘ, and the menu left open while columns are ticked one after
 *  another. */
function ColumnsMenu() {
  const { columns } = useTracePrefs();
  const [opened, setOpened] = useState<ReadonlySet<ColumnGroup>>(() => new Set());
  const shown = (key: TraceColumn) => columns.includes(key);
  const flip = (key: TraceColumn) =>
    setTracePrefs({
      columns: orderedColumns(shown(key) ? columns.filter((c) => c !== key) : [...columns, key]),
    });
  const toggleGroup = (group: ColumnGroup) =>
    setOpened((was) => {
      const next = new Set(was);
      if (next.has(group)) next.delete(group);
      else next.add(group);
      return next;
    });

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline">
          <Columns3Icon />
          Columns
          <Badge variant="secondary" className="px-1.5 tabular-nums">
            {columns.length + 1}/{TRACE_COLUMNS.length + 1}
          </Badge>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="max-h-96 w-64">
        <ColumnItem label={TRACE_ID_COLUMN.label} hint={TRACE_ID_COLUMN.hint} checked disabled />
        {COLUMN_MENU.map((block) => {
          if (block.kind === "column") {
            const { column } = block;
            return (
              <ColumnItem
                key={column.key}
                label={column.label}
                hint={column.hint}
                checked={shown(column.key)}
                onToggle={() => flip(column.key)}
              />
            );
          }
          const open = opened.has(block.group);
          const count = block.columns.filter((c) => shown(c.key)).length;
          return (
            <Fragment key={block.group}>
              <DropdownMenuItem
                aria-expanded={open}
                className="bg-muted/60 font-medium [[data-slot=dropdown-menu-item]+&]:mt-1"
                onSelect={(event) => {
                  event.preventDefault();
                  toggleGroup(block.group);
                }}
              >
                <ChevronRightIcon
                  className={cn("text-muted-foreground transition-transform", open && "rotate-90")}
                />
                {block.label}
                <span className="font-normal text-muted-foreground tabular-nums">
                  ({count}/{block.columns.length})
                </span>
              </DropdownMenuItem>
              {open &&
                block.columns.map((column) => (
                  <ColumnItem
                    key={column.key}
                    label={column.label}
                    hint={column.hint}
                    checked={shown(column.key)}
                    onToggle={() => flip(column.key)}
                    inset
                  />
                ))}
            </Fragment>
          );
        })}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => setTracePrefs({ columns: DEFAULT_COLUMNS })}>
          Back to the defaults
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** One column in the menu: a box ticked while it shows, and what it means
 *  behind an ⓘ where its heading does not say. A column always shown is
 *  ticked and greyed, as Langfuse greys its own. */
function ColumnItem({
  label,
  hint,
  checked,
  disabled = false,
  onToggle,
  inset = false,
}: {
  label: string;
  hint?: string;
  checked: boolean;
  disabled?: boolean;
  onToggle?: () => void;
  inset?: boolean;
}) {
  return (
    <DropdownMenuItem
      role="menuitemcheckbox"
      aria-checked={checked}
      disabled={disabled}
      className={cn(inset && "pl-7")}
      onSelect={(event) => {
        event.preventDefault();
        onToggle?.();
      }}
    >
      <Checkbox
        checked={checked}
        disabled={disabled}
        tabIndex={-1}
        aria-hidden="true"
        className="pointer-events-none"
      />
      <span className="flex-1 truncate">{label}</span>
      {hint && <Hint text={hint} />}
    </DropdownMenuItem>
  );
}

/** What a column means, on hover, beside its name. */
function Hint({ text }: { text: string }): ReactNode {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="pointer-events-auto text-muted-foreground" aria-label={text}>
          <InfoIcon className="size-3.5" />
        </span>
      </TooltipTrigger>
      <TooltipContent side="right" className="max-w-56">
        {text}
      </TooltipContent>
    </Tooltip>
  );
}

const DENSITY_LABEL: Record<Density, string> = {
  comfortable: "Comfortable",
  compact: "Compact",
};

/** How tightly the rows sit. */
function DensityMenu() {
  const { density } = useTracePrefs();
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button variant="outline" size="icon" aria-label="Row density">
              <Rows3Icon />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent>Row density</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel>Rows</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={density}
          onValueChange={(value) => setTracePrefs({ density: value as Density })}
        >
          {DENSITIES.map((d) => (
            <DropdownMenuRadioItem key={d} value={d}>
              {DENSITY_LABEL[d]}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** The list as a file: every trace its filters admit, sorted as it is, up
 *  to the server's cap, which the menu names when the list is past it. */
function DownloadMenu({
  organization,
  slug,
  query,
  total,
}: {
  organization: string;
  slug: string;
  query: TraceQuery;
  total: number;
}) {
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button variant="outline" size="icon" aria-label="Download">
              <DownloadIcon />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent>Download</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-64">
        <DropdownMenuLabel className="font-normal text-muted-foreground">
          {total > DOWNLOAD_MAX
            ? `The first ${formatCount(DOWNLOAD_MAX)} of ${formatCount(total)} traces`
            : `${formatCount(total)} ${total === 1 ? "trace" : "traces"}, as the list is sorted`}
        </DropdownMenuLabel>
        <DropdownMenuItem asChild>
          <a href={downloadHref(organization, slug, query, "csv")} download>
            CSV, for a spreadsheet
          </a>
        </DropdownMenuItem>
        <DropdownMenuItem asChild>
          <a href={downloadHref(organization, slug, query, "json")} download>
            JSON
          </a>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
