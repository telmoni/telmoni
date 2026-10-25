"use client";

import { startTransition, useId, useState, useTransition } from "react";
import {
  BookmarkIcon,
  CheckIcon,
  ChevronDownIcon,
  PencilIcon,
  PlusIcon,
  SaveIcon,
  Trash2Icon,
} from "lucide-react";
import Link from "next/link";
import { toast } from "sonner";

import {
  deleteTraceViewAction,
  saveTraceViewAction,
  updateTraceViewAction,
} from "@/app/(app)/[organization]/[project]/traces/actions";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { TraceQuery, TraceView } from "@/lib/server/entities/traces";
import { orderedColumns, showsView, viewHref, viewQueryOf, type TraceUi } from "@/lib/traces";
import { ViewKind } from "@/lib/types/enums";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";
import { cn } from "@/lib/utils";

/** The longest a view's name is, as the server takes it. */
const NAME_MAX = 60;

type Props = {
  organization: string;
  slug: string;
  projectId: string;
  query: TraceQuery;
  ui: TraceUi;
  /** The project's views, or `null` where they could not be read. */
  views: TraceView[] | null;
  /** Whether the person may save, save over and delete them: an owner or an
   *  admin, as the server holds. */
  canSave: boolean;
};

// The project's saved views: picked from a
// menu, each opening the list on its filters, sort and window with its
// columns; the one the page was opened from named on the button, starred
// once the list has moved off it. Every member applies them; owners and
// admins save them, for everyone on the project.
export function ViewsMenu({ organization, slug, projectId, query, ui, views, canSave }: Props) {
  const prefs = useTracePrefs();
  const [, start] = useTransition();
  const [naming, setNaming] = useState<"new" | "rename" | null>(null);
  const [deleting, setDeleting] = useState(false);

  const current = views?.find((v) => v.id === ui.view);
  const columnsMoved =
    current !== undefined && current.columns.join() !== orderedColumns(prefs.columns).join();
  const edited = current !== undefined && (!showsView(query, current) || columnsMoved);

  const saveOver = (view: TraceView) =>
    start(async () => {
      const result = await updateTraceViewAction(projectId, view.id, {
        name: view.name,
        kind: ViewKind.List,
        query: viewQueryOf(query),
        columns: orderedColumns(prefs.columns),
      });
      if (result.status === "error") {
        toast.error(result.error);
        return;
      }
      toast.success(`Saved “${view.name}”`);
    });

  // A view deleted from the page it opened takes the page back to the list it
  // showed in the action itself, whose redirect rejects the call and goes on
  // to the router; only a refusal answers here.
  const remove = (view: TraceView) =>
    start(async () => {
      const result = await deleteTraceViewAction(projectId, view.id);
      if (result.status === "error") {
        toast.error(result.error);
        return;
      }
      toast.success(`Deleted “${view.name}”`);
    });

  return (
    <>
      {/* Not modal: an item here opens a dialog, and a modal menu closing as
          the dialog opens would leave the page's pointer events locked. */}
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          {/* Never disabled while a save is on its way: the trigger holds
              the focus the menu hands back on closing, and a disabled
              control drops it to the page. */}
          <Button
            variant="outline"
            aria-label={current ? `Saved view: ${current.name}` : "Saved views"}
          >
            <BookmarkIcon />
            <span className="max-w-40 truncate">{current ? current.name : "Views"}</span>
            {edited && (
              <span aria-hidden="true" title="Changed since it was saved">
                *
              </span>
            )}
            <Badge variant="secondary" className="px-1.5 tabular-nums">
              {views?.length ?? 0}
            </Badge>
            <ChevronDownIcon className="text-muted-foreground" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-72">
          <DropdownMenuLabel>Saved views</DropdownMenuLabel>
          {views === null ? (
            <DropdownMenuItem disabled>The views could not be read</DropdownMenuItem>
          ) : views.length === 0 ? (
            <DropdownMenuItem disabled>No saved views yet</DropdownMenuItem>
          ) : (
            // Each a link, so a view opens as any page does — in a new tab as
            // well — with the list kept where it is scrolled to. Its columns
            // are the browser's, set on any click, a new tab's included.
            views.map((view) => {
              const applied = view.id === current?.id;
              return (
                <DropdownMenuItem key={view.id} asChild>
                  <Link
                    href={viewHref(organization, slug, view, { display: ui.display })}
                    scroll={false}
                    onClick={() => setTracePrefs({ columns: orderedColumns(view.columns) })}
                    aria-current={applied ? "true" : undefined}
                  >
                    <CheckIcon className={cn(!applied && "invisible")} aria-hidden="true" />
                    <span className="truncate">{view.name}</span>
                  </Link>
                </DropdownMenuItem>
              );
            })
          )}
          <DropdownMenuSeparator />
          <DropdownMenuItem disabled={!canSave || views === null} onSelect={() => setNaming("new")}>
            <PlusIcon />
            Save as a new view…
          </DropdownMenuItem>
          {current && (
            <>
              <DropdownMenuItem disabled={!canSave || !edited} onSelect={() => saveOver(current)}>
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
          {!canSave && (
            <p className="px-2 py-1.5 text-xs text-muted-foreground">
              Owners and admins save views; everyone on the project applies them.
            </p>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      <NameDialog
        key={`${naming ?? "closed"}:${current?.id ?? ""}`}
        open={naming !== null}
        onOpenChange={(open) => !open && setNaming(null)}
        title={naming === "rename" ? "Rename the view" : "Save as a new view"}
        description={
          naming === "rename"
            ? "The view keeps its filters, sort and columns."
            : "Everyone on the project can open it from Views: this list's filters, sort, window and columns, under a name."
        }
        initial={naming === "rename" ? (current?.name ?? "") : ""}
        submitLabel={naming === "rename" ? "Rename" : "Save view"}
        onSubmit={async (name) => {
          if (naming === "rename" && current) {
            const result = await updateTraceViewAction(projectId, current.id, {
              name,
              kind: ViewKind.List,
              query: current.query,
              columns: current.columns,
            });
            if (result.status === "error") return result.error;
            toast.success(`Renamed to “${name}”`);
            return null;
          }
          // A view saved opens the list on it in the action itself, whose
          // redirect goes on to the router; only a refusal answers here.
          const result = await saveTraceViewAction(projectId, {
            name,
            kind: ViewKind.List,
            query: viewQueryOf(query),
            columns: orderedColumns(prefs.columns),
          });
          if (result.status === "error") return result.error;
          toast.success(`Saved “${name}”`);
          return null;
        }}
      />

      {current && (
        <ConfirmDialog
          open={deleting}
          onOpenChange={setDeleting}
          title={`Delete “${current.name}”?`}
          description="It goes for everyone on the project. The traces it showed are not touched."
          confirmLabel="Delete view"
          destructive
          onConfirm={() => remove(current)}
        />
      )}
    </>
  );
}

/** A view's name, asked for: saved, or the server's refusal shown beside
 *  the field — a name taken, a project at its hundred views. The layouts
 *  menu asks the same way (`layout-controls.tsx`). */
export function NameDialog({
  open,
  onOpenChange,
  title,
  description,
  initial,
  submitLabel,
  onSubmit,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  initial: string;
  submitLabel: string;
  /** Answers the refusal to show, or `null` once saved. */
  onSubmit: (name: string) => Promise<string | null>;
}) {
  const nameId = useId();
  const [name, setName] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();
  const trimmed = name.trim();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <form
          className="contents"
          onSubmit={(event) => {
            event.preventDefault();
            if (!trimmed) return;
            start(async () => {
              const refused = await onSubmit(trimmed);
              if (refused) {
                setError(refused);
                return;
              }
              // With the list the save drew again: after the `await` it would
              // be an update of its own, and the dialog would close a frame
              // before the menu shows the change.
              startTransition(() => onOpenChange(false));
            });
          }}
        >
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <DialogDescription>{description}</DialogDescription>
            <div className="grid gap-2">
              <Label htmlFor={nameId}>Name</Label>
              <Input
                id={nameId}
                value={name}
                maxLength={NAME_MAX}
                autoComplete="off"
                autoFocus
                aria-invalid={error !== null}
                onChange={(event) => {
                  setName(event.target.value);
                  setError(null);
                }}
              />
              {error && <p className="text-sm text-destructive">{error}</p>}
            </div>
          </DialogBody>
          <DialogFooter>
            <Button type="submit" disabled={pending || !trimmed}>
              {submitLabel}
            </Button>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
