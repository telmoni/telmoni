"use client";

import { Fragment, useId, useRef, useState, useTransition } from "react";
import {
  ChevronRightIcon,
  CircleXIcon,
  CoinsIcon,
  Columns3Icon,
  DownloadIcon,
  RefreshCwIcon,
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
import { Toggle } from "@/components/ui/toggle";
import {
  ColumnHint,
  DownloadItems,
  columnsCounted,
  toggledColumn,
} from "@/components/traces/list-table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { WindowPicker } from "@/components/window-picker";
import type { SessionQuery } from "@/lib/server/entities/sessions";
import type { ListQuery, TraceQuery } from "@/lib/server/entities/traces";
import type { UserQuery } from "@/lib/server/entities/users";
import { SESSIONS } from "@/lib/sessions";
import { USERS } from "@/lib/users";
import {
  COLUMNS_PREF,
  type TracePrefs,
} from "@/lib/trace-prefs";
import {
  DOWNLOAD_MAX,
  ROW_NOUN,
  TRACES,
  counted,
  formatCount,
  listDownloadHref,
  ordered,
  type ColumnBlock,
  type ColumnGroup,
  type ListName,
  type ListSpec,
  type ListTable,
  type Preset,
} from "@/lib/traces";
import { toggled } from "@/lib/trace-tree";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";
import { useOverflow } from "@/lib/use-overflow";
import { cn } from "@/lib/utils";

/** Each ready-made query's icon. */
const PRESET_ICON: Record<Preset["id"], LucideIcon> = {
  errors: CircleXIcon,
  slow: TimerIcon,
  tokens: CoinsIcon,
};

/** Each list's columns menu, and its rows: its own scaffold's. */
const TABLES: Record<ListName, { table: ListTable; menu: readonly ColumnBlock[] }> = {
  traces: TRACES,
  sessions: SESSIONS,
  users: USERS,
};

/** A list and its query. */
type ListToolbarProps = {
  organization: string;
  slug: string;
  /** How many rows the filters admit. */
  total: number;
  /** The window's days, the toolbar's first control. */
  days: number;
} & (
  | { list: "traces"; query: TraceQuery }
  | { list: "sessions"; query: SessionQuery }
  | { list: "users"; query: UserQuery }
);

// A list's toolbar, one row of shadcn controls at the title row's end: the
// refresh and the window, the ready-made queries, then its columns and its
// download (`ListControls`). A control that changes what the list holds
// changes its URL, and one that changes only how it looks is kept in the
// browser (`lib/trace-prefs.ts`).
export function ListToolbar({ total, ...props }: ListToolbarProps) {
  const { organization, slug, days } = props;
  const { go, pending } = useListNavigation();
  const strip = useRef<HTMLDivElement>(null);
  const overflow = useOverflow(strip);
  // Each list's own scaffold, by its own query's sorts.
  const presets =
    props.list === "traces" ? (
      <Presets spec={TRACES} query={props.query} go={go} at={[organization, slug]} />
    ) : props.list === "sessions" ? (
      <Presets spec={SESSIONS} query={props.query} go={go} at={[organization, slug]} />
    ) : (
      <Presets spec={USERS} query={props.query} go={go} at={[organization, slug]} />
    );

  return (
    // The room the title leaves, its controls at its end; too narrow for them,
    // it scrolls from the first (a safe end, so none is pushed out of reach),
    // its padding keeping the focus outlines from being clipped. No scrollbar: a
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
  at: [organization, slug],
}: {
  spec: ListSpec<K, S>;
  query: ListQuery<S>;
  go: (href: string) => void;
  at: [organization: string, slug: string];
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
              go(spec.href(organization, slug, spec.presetToggled(query, preset.id)))
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

/** A refresh now. The list draws itself as traces land, so it needs no timer. */
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
            : `${counted(total, one, many)}, as the list is sorted`}
        </DropdownMenuLabel>
        <DownloadItems href={(format) => listDownloadHref(list, organization, slug, query, format)} />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
