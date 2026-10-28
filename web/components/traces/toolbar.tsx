"use client";

import { Fragment, useEffect, useId, useState, useTransition } from "react";
import {
  ChartColumnIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  CircleXIcon,
  CoinsIcon,
  Columns3Icon,
  DownloadIcon,
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
import {
  ColumnHint,
  DownloadItems,
  columnsCounted,
  toggledColumn,
} from "@/components/traces/list-table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { WindowPicker } from "@/components/window-picker";
import type { ListQuery, TraceQuery, TraceView } from "@/lib/server/entities/traces";
import { SESSION_COLUMN_MENU, SESSION_TABLE } from "@/lib/sessions";
import {
  COLUMNS_PREF,
  DENSITIES,
  REFRESH_INTERVALS,
  refreshLabel,
  type Density,
  type TracePrefs,
} from "@/lib/trace-prefs";
import {
  COLUMN_MENU,
  DOWNLOAD_MAX,
  ROW_NOUN,
  TRACE_PRESETS,
  TRACE_TABLE,
  formatCount,
  listDownloadHref,
  ordered,
  presetActive,
  presetToggled,
  tracesHref,
  type ColumnBlock,
  type ColumnGroup,
  type ListName,
  type ListTable,
  type Preset,
  type TraceUi,
} from "@/lib/traces";
import { toggled } from "@/lib/trace-tree";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";
import { USER_COLUMN_MENU, USER_TABLE } from "@/lib/users";
import { isOneOf } from "@/lib/one-of";
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

/** Each ready-made query's icon. */
const PRESET_ICON: Record<Preset["id"], LucideIcon> = {
  errors: CircleXIcon,
  slow: TimerIcon,
  tokens: CoinsIcon,
};

/** Each list's columns menu, and its rows. */
const TABLES: Record<ListName, { table: ListTable; menu: readonly ColumnBlock[] }> = {
  traces: { table: TRACE_TABLE, menu: COLUMN_MENU },
  sessions: { table: SESSION_TABLE, menu: SESSION_COLUMN_MENU },
  users: { table: USER_TABLE, menu: USER_COLUMN_MENU },
};

// The trace list's toolbar, on one row of shadcn controls: the ready-made
// queries, the saved views and the table or the charts on the left; on the
// right what every list has (`ListControls`). The window and the refresh
// stand on the title's row (`ListHeaderControls`). A control that changes
// what the list holds changes its URL, and one that changes only how it
// looks is kept in the browser (`lib/trace-prefs.ts`).
export function TraceToolbar(props: Props) {
  const { organization, slug, query, ui } = props;
  const { go, pending } = useListNavigation();
  const href = (next: TraceQuery, nextUi: TraceUi = ui) =>
    tracesHref(organization, slug, next, {}, nextUi);

  return (
    <div
      className="flex flex-wrap items-center gap-3.5"
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

      <ListControls list="traces" organization={organization} slug={slug} query={query} total={props.total} />
    </div>
  );
}

/** The sessions' or the users' toolbar: what every list has, on the right,
 *  as the trace list's toolbar has it. */
export function ListToolbar(props: ListControlsProps) {
  return (
    <div className="flex flex-wrap items-center gap-3.5">
      <ListControls {...props} />
    </div>
  );
}

type ListControlsProps = {
  list: ListName;
  organization: string;
  slug: string;
  query: ListQuery<string>;
  /** How many rows the filters admit. */
  total: number;
};

/** A list's columns, its rows' density and its download. */
function ListControls({ list, organization, slug, query, total }: ListControlsProps) {
  return (
    <div className="ml-auto flex items-center gap-3.5">
      <ColumnsMenu list={list} />
      <DensityMenu />
      <DownloadMenu list={list} organization={organization} slug={slug} query={query} total={total} />
    </div>
  );
}

// The window a list reads and its refresh, on the title's row.
export function ListHeaderControls({ days }: { days: number }) {
  return (
    <div className="flex items-center gap-3.5">
      <WindowPicker days={days} />
      <RefreshControl />
    </div>
  );
}

/** A refresh now, and how often the page reads itself again: only while it
 *  is in view, so a tab left open asks nothing of the server. The list
 *  counts the runs that land in a bar above itself (`NewTraces`) and is
 *  drawn when asked, so the interval is for a console whose live events are
 *  paused. */
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
            <ChevronDownIcon className="text-muted-foreground" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuLabel>Refresh every</DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={String(refresh)}
            onValueChange={(value) => {
              const refresh = Number(value);
              if (isOneOf(REFRESH_INTERVALS, refresh)) setTracePrefs({ refresh });
            }}
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

/** Which columns a list's table shows: the row's id ticked for good, the
 *  counts and the tokens each a group under its heading, what a column means
 *  behind its ⓘ, and the menu left open while columns are ticked. */
function ColumnsMenu({ list }: { list: ListName }) {
  const prefs = useTracePrefs();
  const { table, menu } = TABLES[list];
  const pref = COLUMNS_PREF[list];
  const columns: readonly string[] = prefs[pref];
  const [opened, setOpened] = useState<ReadonlySet<ColumnGroup>>(() => new Set());
  const shown = (key: string) => columns.includes(key);
  // Held to the list's own columns, in its order, so the cast names what they are.
  const keep = (keys: readonly string[]) =>
    setTracePrefs({ [pref]: ordered(table.columns, keys) } as Partial<TracePrefs>);
  const flip = (key: string) => keep(toggledColumn(columns, key));
  const toggleGroup = (group: ColumnGroup) => setOpened((was) => toggled(was, group));

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline">
          <Columns3Icon />
          Columns
          <Badge variant="secondary" className="px-1.5 tabular-nums">
            {columnsCounted(columns.length, table.columns.length)}
          </Badge>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-64">
        <ColumnItem label={table.id.label} hint={table.id.hint} checked disabled />
        {menu.map((block) => {
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
                  className={cn("transition-transform", open && "rotate-90")}
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
        <DropdownMenuItem onSelect={() => keep(table.defaults)}>
          Back to the defaults
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** One column in the menu: a box ticked while it shows, and what it means
 *  behind an ⓘ where its heading does not say. A column always shown is
 *  ticked and greyed. */
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
  const hintId = useId();
  return (
    <DropdownMenuItem
      role="menuitemcheckbox"
      aria-checked={checked}
      aria-describedby={hint ? hintId : undefined}
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
      {hint && <ColumnHint id={hintId} text={hint} />}
    </DropdownMenuItem>
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
          onValueChange={(value) => {
            if (isOneOf(DENSITIES, value)) setTracePrefs({ density: value });
          }}
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

/** The list as a file: every row its filters admit, sorted as it is, up to
 *  the server's cap, which the menu names when the list is past it. */
function DownloadMenu({ list, organization, slug, query, total }: ListControlsProps) {
  const [one, many] = ROW_NOUN[list];
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
            ? `The first ${formatCount(DOWNLOAD_MAX)} of ${formatCount(total)} ${many}`
            : `${formatCount(total)} ${total === 1 ? one : many}, as the list is sorted`}
        </DropdownMenuLabel>
        <DownloadItems href={(format) => listDownloadHref(list, organization, slug, query, format)} />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
