"use client";

import { Fragment, type ReactNode, useId, useRef, useState, useTransition } from "react";
import {
  ChartColumnIcon,
  ChevronRightIcon,
  CircleXIcon,
  CoinsIcon,
  Columns3Icon,
  DownloadIcon,
  GroupIcon,
  RefreshCwIcon,
  TableIcon,
  TimerIcon,
  type LucideIcon,
} from "lucide-react";
import { useRouter } from "next/navigation";

import { useListNavigation } from "@/components/traces/navigate";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import { Checkbox } from "@/components/ui/checkbox";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
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
import type { ListQuery, TraceQuery } from "@/lib/server/entities/traces";
import {
  COLUMNS_PREF,
  type TracePrefs,
} from "@/lib/trace-prefs";
import {
  COLUMN_MENU,
  DOWNLOAD_MAX,
  ROW_NOUN,
  TRACES,
  TRACE_TABLE,
  formatCount,
  groupOf,
  groupedBy,
  listDownloadHref,
  ordered,
  tracesHref,
  type ColumnBlock,
  type ColumnGroup,
  type Group,
  type ListName,
  type ListSpec,
  type ListTable,
  type Preset,
  type TraceUi,
} from "@/lib/traces";
import { toggled } from "@/lib/trace-tree";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";
import { useOverflow } from "@/lib/use-overflow";
import { cn } from "@/lib/utils";

type Common = {
  organization: string;
  slug: string;
  ui: TraceUi;
  /** How many rows the filters admit. */
  total: number;
  /** The window's days, the toolbar's first control. */
  days: number;
};

type Props = Common & { query: TraceQuery };

/** Each ready-made query's icon. */
const PRESET_ICON: Record<Preset["id"], LucideIcon> = {
  errors: CircleXIcon,
  slow: TimerIcon,
  tokens: CoinsIcon,
};

/** Each list's columns menu, and its rows. */
const TABLES: Record<ListName, { table: ListTable; menu: readonly ColumnBlock[] }> = {
  traces: { table: TRACE_TABLE, menu: COLUMN_MENU },
};

/** What the list is grouped by, as its control names each. */
const GROUP_LABEL: Record<Group | "none", string> = {
  none: "No grouping",
  session: "By session",
  user: "By user",
};

// The trace list's toolbar: every list's (`ListToolbar`), with its grouping
// and the table or the charts beside its ready-made queries.
export function TraceToolbar(props: Props) {
  const { organization, slug, query, ui } = props;
  return (
    <ListToolbar
      {...props}
      list="traces"
      extra={(go) => (
        <>
          <Select
            value={groupOf(query) ?? "none"}
            onValueChange={(value) =>
              go(
                tracesHref(
                  organization,
                  slug,
                  groupedBy(query, value === "session" || value === "user" ? value : undefined),
                  {},
                  ui,
                ),
              )
            }
          >
            <SelectTrigger aria-label="Group by">
              <GroupIcon />
              <SelectValue />
            </SelectTrigger>
            <SelectContent position="popper" align="end">
              {(["none", "session", "user"] as const).map((value) => (
                <SelectItem key={value} value={value}>
                  {GROUP_LABEL[value]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Tabs
            value={ui.display ?? "table"}
            onValueChange={(display) =>
              go(
                tracesHref(organization, slug, query, {}, {
                  ...ui,
                  display: display === "chart" ? "chart" : undefined,
                }),
              )
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
        </>
      )}
    />
  );
}

/** A list and its query. */
type ListToolbarProps = Common & {
  list: "traces";
  query: TraceQuery;
  /** What the list shows beside its ready-made queries, given the toolbar's
   *  way to navigate so the toolbar reads busy while it does. */
  extra?: (go: (href: string) => void) => ReactNode;
};

// A list's toolbar, one row of shadcn controls at the title row's end: the
// refresh and the window, the ready-made queries and the list's own, then
// what every list has (`ListControls`). A
// control that changes what the list holds changes its URL, and one that
// changes only how it looks is kept in the browser (`lib/trace-prefs.ts`).
export function ListToolbar({ extra, total, ...props }: ListToolbarProps) {
  const { organization, slug, ui, days } = props;
  const { go, pending } = useListNavigation();
  const strip = useRef<HTMLDivElement>(null);
  const overflow = useOverflow(strip);
  const presets = <Presets spec={TRACES} query={props.query} go={go} at={[organization, slug, ui]} />;

  return (
    // The room the title leaves, its controls at its end; too narrow for them,
    // it scrolls from the first (a safe end, so none is pushed out of reach),
    // its padding keeping the focus rings from being clipped. No scrollbar: a
    // bar under one row of controls reads as a frame around them, so the edge
    // with more beyond it fades instead (`useOverflow`).
    <div
      ref={strip}
      className="relative -m-1 min-w-0 flex-1"
      data-overflow={overflow || undefined}
    >
      <ButtonGroup
        data-scroll-x=""
        className="justify-end-safe overflow-x-auto p-1 [&>*]:shrink-0"
        aria-busy={pending}
        data-pending={pending ? "" : undefined}
      >
        <RefreshControl />
        <WindowPicker days={days} />
        {presets}
        {extra?.((href) => go(href))}
        <ListControls
          list={props.list}
          organization={organization}
          slug={slug}
          query={props.query}
          total={total}
        />
      </ButtonGroup>
    </div>
  );
}

/** A list's ready-made queries, each a toggle that sets or clears it. */
function Presets<K extends string, S extends string>({
  spec,
  query,
  go,
  at: [organization, slug, ui],
}: {
  spec: ListSpec<K, S>;
  query: ListQuery<S>;
  go: (href: string) => void;
  at: [organization: string, slug: string, ui: TraceUi];
}) {
  return spec.presets.map((preset) => {
    const Icon = PRESET_ICON[preset.id];
    return (
      <Tooltip key={preset.id}>
        <TooltipTrigger asChild>
          <Toggle
            variant="outline"
            pressed={spec.presetActive(query, preset.id)}
            onPressedChange={() =>
              go(spec.href(organization, slug, spec.presetToggled(query, preset.id), {}, ui))
            }
          >
            <Icon />
            {preset.label}
          </Toggle>
        </TooltipTrigger>
        <TooltipContent>{preset.hint}</TooltipContent>
      </Tooltip>
    );
  });
}

type ListControlsProps = {
  list: ListName;
  organization: string;
  slug: string;
  query: ListQuery<string>;
  /** How many rows the filters admit. */
  total: number;
};

/** A list's columns and its download; its rows' density is the General
 *  settings page's. */
function ListControls({ list, organization, slug, query, total }: ListControlsProps) {
  return (
    <ButtonGroup>
      <ColumnsMenu list={list} />
      <DownloadMenu list={list} organization={organization} slug={slug} query={query} total={total} />
    </ButtonGroup>
  );
}

/** A refresh now. The list counts the runs that land in a bar above itself
 *  (`NewTraces`) and draws them when asked, so it needs no timer. */
function RefreshControl() {
  const router = useRouter();
  const [refreshing, start] = useTransition();

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="outline"
          size="icon"
          aria-label="Refresh"
          onClick={() => start(() => router.refresh())}
        >
          <RefreshCwIcon className={cn(refreshing && "animate-spin")} />
        </Button>
      </TooltipTrigger>
      <TooltipContent>Refresh</TooltipContent>
    </Tooltip>
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
          <Badge variant="secondary" className="tabular-nums">
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
      className={cn(inset && "pl-row-text")}
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
        <DropdownMenuLabel>
          {total > DOWNLOAD_MAX
            ? `The first ${formatCount(DOWNLOAD_MAX)} of ${formatCount(total)} ${many}`
            : `${formatCount(total)} ${total === 1 ? one : many}, as the list is sorted`}
        </DropdownMenuLabel>
        <DownloadItems href={(format) => listDownloadHref(list, organization, slug, query, format)} />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
