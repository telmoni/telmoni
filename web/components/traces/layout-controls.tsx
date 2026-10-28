"use client";

import { useId, useState, useTransition, type ComponentProps, type ReactNode } from "react";
import {
  BookmarkIcon,
  CheckIcon,
  ChevronDownIcon,
  InfoIcon,
  PencilIcon,
  PlusIcon,
  SaveIcon,
  SlidersHorizontalIcon,
  Trash2Icon,
  XIcon,
} from "lucide-react";
import Link from "next/link";
import { toast } from "sonner";

import {
  deleteTraceViewAction,
  saveTraceViewAction,
  setSessionLayoutAction,
  updateTraceViewAction,
} from "@/app/(app)/[organization]/[project]/traces/actions";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { NameDialog } from "@/components/traces/views-menu";
import { useLayout, type LayoutPage } from "@/components/traces/use-layout";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import {
  DEFAULT_LAYOUT_NAME,
  PINNED_MAX,
  addName,
  choiceOfLayout,
  layoutQueryOf,
  withLayoutParams,
  type LayoutFilter,
} from "@/lib/layout";
import type { TraceLayout } from "@/lib/server/data";
import { KIND_LABEL, STATUS_LABEL, TRACE_COLUMNS, TRACE_ID_COLUMN, orderedColumns } from "@/lib/traces";
import {
  LayoutDepth,
  LayoutOpen,
  SpanKind,
  SpanStatus,
  ViewKind,
  type TraceColumn,
} from "@/lib/types/enums";
import { cn } from "@/lib/utils";

// The layout a session's page and a trace's open in, in the page header's
// action: Customize, which changes any option from the default or from the
// saved layout the page opened on, marked as changed with a way back; and
// Layouts, where a customization is kept under a name for everyone on the
// project, as the trace list's views are, and where owners and admins set
// what the project opens in.

type Props = LayoutPage & {
  organization: string;
  slug: string;
  projectId: string;
  /** Whether the person may save, save over and delete layouts, and set the
   *  project's default: an owner or an admin, as the server holds. */
  canSave: boolean;
};

const DEPTH_LABEL: Record<LayoutDepth, string> = {
  [LayoutDepth.Trace]: "The trace alone",
  [LayoutDepth.Steps]: "Its steps",
  [LayoutDepth.Calls]: "Its steps and calls",
};

const OPEN_LABEL: Record<LayoutOpen, string> = {
  [LayoutOpen.None]: "None",
  [LayoutOpen.Failed]: "The failed",
  [LayoutOpen.All]: "All",
};

/** What the project's default radio says for the default layout, beside
 *  the saved layouts' ids. */
const DEFAULT_CHOICE = "default";

/** The kinds of span a layout's filter picks among: every kind but the
 *  run's own, which is the trace. */
const FILTERED_KINDS: readonly SpanKind[] = [
  SpanKind.Step,
  SpanKind.ModelCall,
  SpanKind.ToolCall,
  SpanKind.Span,
];

export function LayoutControls(props: Props) {
  const layout = useLayout(props);
  const from = layout.current ? `“${layout.current.name}”` : DEFAULT_LAYOUT_NAME;
  return (
    <TooltipProvider>
      <div className="flex flex-wrap items-center gap-2">
        <CustomizeMenu layout={layout} from={from} />
        <LayoutsMenu {...props} layout={layout} />
      </div>
    </TooltipProvider>
  );
}

type Layout = ReturnType<typeof useLayout>;

/** Every option: the depth, what starts open, the timeline, the columns,
 *  the spans pinned, the calls shown, and the messages and scores, kept now
 *  and shown once content is kept and traces are scored. */
function CustomizeMenu({ layout, from }: { layout: Layout; from: string }) {
  const { options, setOptions, isEdited, reset } = layout;
  const [pins, setPins] = useState(false);
  const [calls, setCalls] = useState(false);
  const shown = (key: TraceColumn) => options.columns.includes(key);
  const flip = (key: TraceColumn) =>
    setOptions({
      columns: orderedColumns(
        shown(key) ? options.columns.filter((c) => c !== key) : [...options.columns, key],
      ),
    });
  // The menu stays open while options are changed one after another.
  const keep = (event: Event) => event.preventDefault();
  const filtered =
    options.filter.kind.length + options.filter.status.length + options.filter.name.length;

  return (
    <>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" aria-label={isEdited ? "Customize the layout, changed" : "Customize the layout"}>
            <SlidersHorizontalIcon />
            Customize
            {isEdited && <span aria-hidden="true">*</span>}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-80">
          <ChoiceSub
            label="How far a trace opens"
            labels={DEPTH_LABEL}
            value={options.depth}
            onChange={(depth) => setOptions({ depth })}
            onSelect={keep}
          />
          <ChoiceSub
            label="Which start open"
            labels={OPEN_LABEL}
            value={options.open}
            onChange={(open) => setOptions({ open })}
            onSelect={keep}
          />
          <DropdownMenuSeparator />
          <HintedItem
            hint="Each span's bar on the trace's own line of time."
            checked={options.timeline}
            onCheckedChange={(on) => setOptions({ timeline: on === true })}
            onSelect={keep}
          >
            Timeline
          </HintedItem>
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>
              <span className="flex-1 truncate">Columns</span>
              <span className="shrink-0 pl-2 text-xs tabular-nums text-muted-foreground">
                {options.columns.length + 1}/{TRACE_COLUMNS.length + 1}
              </span>
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent className="w-56">
              <DropdownMenuCheckboxItem checked disabled>
                {TRACE_ID_COLUMN.label}
              </DropdownMenuCheckboxItem>
              {TRACE_COLUMNS.map((column) => (
                <HintedItem
                  key={column.key}
                  hint={column.hint}
                  checked={shown(column.key)}
                  onCheckedChange={() => flip(column.key)}
                  onSelect={keep}
                >
                  <span className="flex-1 truncate">{column.label}</span>
                </HintedItem>
              ))}
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          <DropdownMenuItem onSelect={() => setPins(true)}>
            Pinned spans…
            <span className="ml-auto pl-2 text-xs tabular-nums text-muted-foreground">
              {options.pinned.length}/{PINNED_MAX}
            </span>
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => setCalls(true)}>
            Calls shown…
            {filtered > 0 && (
              <span className="ml-auto pl-2 text-xs tabular-nums text-muted-foreground">
                {filtered} {filtered === 1 ? "filter" : "filters"}
              </span>
            )}
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <HintedItem
            hint="The run's own input and output as the turn's two messages, shown once the project keeps content."
            checked={options.messages}
            onCheckedChange={(on) => setOptions({ messages: on === true })}
            onSelect={keep}
          >
            Messages
          </HintedItem>
          <HintedItem
            hint="The trace's scores beside it, shown once traces are scored."
            checked={options.scores}
            onCheckedChange={(on) => setOptions({ scores: on === true })}
            onSelect={keep}
          >
            Scores
          </HintedItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem disabled={!isEdited} onSelect={reset}>
            Back to {from}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <PinsDialog
        key={`pins:${pins}`}
        open={pins}
        onOpenChange={setPins}
        pinned={options.pinned}
        onSave={(pinned) => setOptions({ pinned })}
      />
      <CallsDialog
        key={`calls:${calls}`}
        open={calls}
        onOpenChange={setCalls}
        filter={options.filter}
        onSave={(filter) => setOptions({ filter })}
      />
    </>
  );
}

/** One option of a few, picked in a sub-menu whose row names the one picked,
 *  which keeps the menu short. */
function ChoiceSub<T extends string>({
  label,
  labels,
  value,
  onChange,
  onSelect,
}: {
  label: string;
  /** Each option's name, in the order shown. */
  labels: Record<T, string>;
  value: T;
  onChange: (value: T) => void;
  onSelect: (event: Event) => void;
}) {
  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        <span className="flex-1 truncate">{label}</span>
        <span className="shrink-0 pl-2 text-xs text-muted-foreground">{labels[value]}</span>
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent className="w-56">
        <DropdownMenuRadioGroup value={value} onValueChange={(option) => onChange(option as T)}>
          {(Object.keys(labels) as T[]).map((option) => (
            <DropdownMenuRadioItem key={option} value={option} onSelect={onSelect}>
              {labels[option]}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  );
}

/** A checkbox item described by what it means, which a screen reader reads,
 *  the ⓘ beside its name showing it on hover. */
function HintedItem({
  hint,
  children,
  ...props
}: ComponentProps<typeof DropdownMenuCheckboxItem> & { hint?: string | undefined }): ReactNode {
  const id = useId();
  return (
    <DropdownMenuCheckboxItem aria-describedby={hint ? id : undefined} {...props}>
      {children}
      {hint && (
        <>
          <span id={id} hidden>
            {hint}
          </span>
          <Tooltip>
            <TooltipTrigger asChild>
              <span className="pointer-events-auto ml-auto pl-2 text-muted-foreground" aria-hidden="true">
                <InfoIcon className="size-3.5" />
              </span>
            </TooltipTrigger>
            <TooltipContent side="right" className="max-w-56">
              {hint}
            </TooltipContent>
          </Tooltip>
        </>
      )}
    </DropdownMenuCheckboxItem>
  );
}

/** A list of names as the dialogs take one: the names as chips, each with
 *  its ×, and a field that adds one on Enter. */
function Names({
  id,
  names,
  max,
  placeholder,
  onChange,
}: {
  id: string;
  names: readonly string[];
  max: number;
  placeholder: string;
  onChange: (names: string[]) => void;
}) {
  const [draft, setDraft] = useState("");
  const add = () => {
    const next = addName(names, draft, max);
    if (next.length > names.length) onChange(next);
    setDraft("");
  };
  return (
    <div className="grid gap-2">
      {names.length > 0 && (
        <ul className="flex flex-wrap gap-1.5">
          {names.map((name) => (
            <li
              key={name}
              className="inline-flex max-w-full items-center gap-1 rounded-md border border-border py-0.5 pr-1 pl-2 text-xs"
            >
              <span className="truncate">{name}</span>
              <button
                type="button"
                aria-label={`Remove ${name}`}
                onClick={() => onChange(names.filter((n) => n !== name))}
                className="cursor-pointer rounded-sm text-muted-foreground hover:text-foreground"
              >
                <XIcon className="size-3.5" />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex gap-2">
        <Input
          id={id}
          value={draft}
          placeholder={placeholder}
          maxLength={145}
          autoComplete="off"
          disabled={names.length >= max}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              add();
            }
          }}
        />
        <Button type="button" variant="outline" disabled={!draft.trim() || names.length >= max} onClick={add}>
          Add
        </Button>
      </div>
    </div>
  );
}

/** The span names pinned: up to eight, each shown under every trace while
 *  it is shut. */
function PinsDialog({
  open,
  onOpenChange,
  pinned,
  onSave,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  pinned: readonly string[];
  onSave: (pinned: string[]) => void;
}) {
  const id = useId();
  const [names, setNames] = useState<string[]>([...pinned]);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <form
          className="contents"
          onSubmit={(event) => {
            event.preventDefault();
            onSave(names);
            onOpenChange(false);
          }}
        >
          <DialogHeader>
            <DialogTitle>Pinned spans</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <DialogDescription>
              Up to {PINNED_MAX} span names. Their spans show under each trace while it is
              shut — the intermediate step a reviewer checks — and above the trace&rsquo;s
              own tree.
            </DialogDescription>
            <div className="grid gap-2">
              <Label htmlFor={id}>Span name</Label>
              <Names id={id} names={names} max={PINNED_MAX} placeholder="plan" onChange={setNames} />
            </div>
          </DialogBody>
          <DialogFooter>
            <Button type="submit">Pin</Button>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** The calls a trace shows: by kind, by how they ended, and by name; each
 *  list left empty admits every call. */
function CallsDialog({
  open,
  onOpenChange,
  filter,
  onSave,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  filter: LayoutFilter;
  onSave: (filter: LayoutFilter) => void;
}) {
  const id = useId();
  const [kind, setKind] = useState<SpanKind[]>([...filter.kind]);
  const [status, setStatus] = useState<SpanStatus[]>([...filter.status]);
  const [name, setName] = useState<string[]>([...filter.name]);
  const tick = <T,>(list: T[], value: T, on: boolean) =>
    on ? (list.includes(value) ? list : [...list, value]) : list.filter((v) => v !== value);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <form
          className="contents"
          onSubmit={(event) => {
            event.preventDefault();
            onSave({ kind, status, name });
            onOpenChange(false);
          }}
        >
          <DialogHeader>
            <DialogTitle>Calls shown</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <DialogDescription>
              Only the spans ticked show under a trace, with the spans inside them; nothing
              ticked shows them all. The run&rsquo;s own span always shows.
            </DialogDescription>
            <fieldset className="grid gap-2">
              <legend className="text-sm font-medium">Kind</legend>
              {FILTERED_KINDS.map((k) => (
                <label key={k} className="flex items-center gap-2 text-sm">
                  <Checkbox
                    checked={kind.includes(k)}
                    onCheckedChange={(on) => setKind(tick(kind, k, on === true))}
                  />
                  {KIND_LABEL[k]}
                </label>
              ))}
            </fieldset>
            <fieldset className="grid gap-2">
              <legend className="text-sm font-medium">Status</legend>
              {(Object.values(SpanStatus) as SpanStatus[]).map((s) => (
                <label key={s} className="flex items-center gap-2 text-sm">
                  <Checkbox
                    checked={status.includes(s)}
                    onCheckedChange={(on) => setStatus(tick(status, s, on === true))}
                  />
                  {STATUS_LABEL[s]}
                </label>
              ))}
            </fieldset>
            <div className="grid gap-2">
              <Label htmlFor={id}>Name</Label>
              <Names id={id} names={name} max={50} placeholder="search_docs" onChange={setName} />
            </div>
          </DialogBody>
          <DialogFooter>
            <Button type="submit">Apply</Button>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** The project's saved layouts, as its saved views are: picked from a menu,
 *  each opening the page as it was saved; the one opened starred once the
 *  options move off it. Every member applies them; owners and admins save
 *  them, and set what the project opens in. */
function LayoutsMenu({ projectId, layouts, project, canSave, layout }: Props & { layout: Layout }) {
  const { options, current, isEdited, applyLayout } = layout;
  const [, start] = useTransition();
  const [naming, setNaming] = useState<"new" | "rename" | null>(null);
  const [deleting, setDeleting] = useState(false);
  const input = (name: string) => ({
    name,
    kind: ViewKind.Layout,
    query: layoutQueryOf(options),
    columns: [...options.columns],
  });

  const saveOver = (saved: TraceLayout) =>
    start(async () => {
      const result = await updateTraceViewAction(projectId, saved.id, input(saved.name));
      if (!result.ok) {
        toast.error(result.error);
        return;
      }
      toast.success(`Saved “${saved.name}”`);
    });

  // A layout deleted from the page it opened takes the page back to the
  // layout it would otherwise open in, in the action itself, whose redirect
  // rejects the call and goes on to the router; only a refusal answers here.
  const remove = (saved: TraceLayout) =>
    start(async () => {
      const result = await deleteTraceViewAction(projectId, saved.id);
      if (!result.ok) {
        toast.error(result.error);
        return;
      }
      toast.success(`Deleted “${saved.name}”`);
    });

  const setDefault = (value: string) =>
    start(async () => {
      const viewId = value === DEFAULT_CHOICE ? null : value;
      const result = await setSessionLayoutAction(projectId, { view_id: viewId });
      if (!result.ok) {
        toast.error(result.error);
        return;
      }
      const name = viewId
        ? `“${layouts?.find((l) => l.id === viewId)?.name ?? "the layout"}”`
        : DEFAULT_LAYOUT_NAME;
      toast.success(`The project opens in ${name}`);
    });

  const defaultValue = project ? (project.view_id ?? DEFAULT_CHOICE) : "";

  return (
    <>
      {/* Not modal: an item here opens a dialog, and a modal menu closing as
          the dialog opens would leave the page's pointer events locked. */}
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            variant="outline"
            aria-label={current ? `Saved layout: ${current.name}` : "Saved layouts"}
          >
            <BookmarkIcon />
            <span className="max-w-40 truncate">{current ? current.name : "Layouts"}</span>
            {current && isEdited && (
              <span aria-hidden="true" title="Changed since it was saved">
                *
              </span>
            )}
            <Badge variant="secondary" className="px-1.5 tabular-nums">
              {layouts?.length ?? 0}
            </Badge>
            <ChevronDownIcon className="text-muted-foreground" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-72">
          <DropdownMenuLabel>Saved layouts</DropdownMenuLabel>
          {layouts === null ? (
            <DropdownMenuItem disabled>The layouts could not be read</DropdownMenuItem>
          ) : layouts.length === 0 ? (
            <DropdownMenuItem disabled>No saved layouts yet</DropdownMenuItem>
          ) : (
            // Each a link, so a layout opens as any page does — in a new tab
            // as well; a plain click applies it in place.
            layouts.map((saved) => {
              const applied = saved.id === current?.id;
              return (
                <DropdownMenuItem key={saved.id} asChild>
                  <Link
                    href={withLayoutParams(
                      typeof window === "undefined" ? "" : window.location.search,
                      choiceOfLayout(saved),
                      false,
                    )}
                    scroll={false}
                    onClick={(event) => {
                      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
                        return;
                      }
                      event.preventDefault();
                      applyLayout(saved);
                    }}
                    aria-current={applied ? "true" : undefined}
                  >
                    <CheckIcon className={cn(!applied && "invisible")} aria-hidden="true" />
                    <span className="truncate">{saved.name}</span>
                  </Link>
                </DropdownMenuItem>
              );
            })
          )}
          <DropdownMenuSeparator />
          <DropdownMenuItem disabled={!canSave || layouts === null} onSelect={() => setNaming("new")}>
            <PlusIcon />
            Save as a new layout…
          </DropdownMenuItem>
          {current && (
            <>
              <DropdownMenuItem disabled={!canSave || !isEdited} onSelect={() => saveOver(current)}>
                <SaveIcon />
                <span className="truncate">Save changes to “{current.name}”</span>
              </DropdownMenuItem>
              <DropdownMenuItem disabled={!canSave} onSelect={() => setNaming("rename")}>
                <PencilIcon />
                Rename…
              </DropdownMenuItem>
              <DropdownMenuItem
                variant="destructive"
                disabled={!canSave}
                onSelect={() => setDeleting(true)}
              >
                <Trash2Icon />
                <span className="truncate">Delete “{current.name}”</span>
              </DropdownMenuItem>
            </>
          )}
          <DropdownMenuSeparator />
          <DropdownMenuSub>
            <DropdownMenuSubTrigger disabled={!canSave || project === null}>
              The project opens in
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent className="w-56">
              <DropdownMenuRadioGroup value={defaultValue} onValueChange={setDefault}>
                <DropdownMenuRadioItem value={DEFAULT_CHOICE}>The default layout</DropdownMenuRadioItem>
                {layouts && layouts.length > 0 && <DropdownMenuSeparator />}
                {layouts?.map((saved) => (
                  <DropdownMenuRadioItem key={saved.id} value={saved.id}>
                    <span className="truncate">{saved.name}</span>
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          {!canSave && (
            <p className="px-2 py-1.5 text-xs text-muted-foreground">
              Owners and admins save layouts and set the project&rsquo;s; everyone on the
              project applies them.
            </p>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      <NameDialog
        key={`${naming ?? "closed"}:${current?.id ?? ""}`}
        open={naming !== null}
        onOpenChange={(open) => !open && setNaming(null)}
        title={naming === "rename" ? "Rename the layout" : "Save as a new layout"}
        description={
          naming === "rename"
            ? "The layout keeps its options and columns."
            : "Everyone on the project can open it from Layouts: this page's depth, what starts open, the timeline, columns, pinned spans and calls shown, under a name."
        }
        initial={naming === "rename" ? (current?.name ?? "") : ""}
        submitLabel={naming === "rename" ? "Rename" : "Save layout"}
        onSubmit={async (name) => {
          if (naming === "rename" && current) {
            const result = await updateTraceViewAction(projectId, current.id, {
              name,
              kind: ViewKind.Layout,
              query: current.query,
              columns: current.columns,
            });
            if (!result.ok) return result.error;
            toast.success(`Renamed to “${name}”`);
            return null;
          }
          // A layout saved opens the page on it in the action itself, whose
          // redirect goes on to the router; only a refusal answers here.
          const result = await saveTraceViewAction(projectId, input(name));
          if (!result.ok) return result.error;
          toast.success(`Saved “${name}”`);
          return null;
        }}
      />

      {current && (
        <ConfirmDialog
          open={deleting}
          onOpenChange={setDeleting}
          title={`Delete “${current.name}”?`}
          description={
            project?.view_id === current.id
              ? "It goes for everyone on the project, which then opens in the default layout again."
              : "It goes for everyone on the project. The traces it showed are not touched."
          }
          confirmLabel="Delete layout"
          destructive
          onConfirm={() => remove(current)}
        />
      )}
    </>
  );
}
