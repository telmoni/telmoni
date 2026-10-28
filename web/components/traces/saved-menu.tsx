"use client";

import {
  startTransition,
  useId,
  useState,
  useTransition,
  type MouseEvent,
  type ReactNode,
} from "react";
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
import type { ActionResult } from "@/lib/action-result";
import { cn } from "@/lib/utils";

// The project's saved views and saved layouts are rows of one table, kept
// through one menu: picked, each a link; saved as new, saved over, renamed
// and deleted by owners and admins, for everyone on the project.

/** The longest a saved view's or layout's name is, as the server takes it. */
const NAME_MAX = 60;

/** What the menu's rows share: a saved view of either kind. */
type Saved = { id: string; name: string };

const PLURAL = { view: "Views", layout: "Layouts" } as const;

type Props<T extends Saved> = {
  noun: keyof typeof PLURAL;
  /** The project's, or `null` where they could not be read. */
  saved: readonly T[] | null;
  /** The one the page was opened from. */
  current: T | undefined;
  /** Whether the page has moved off `current` since it was saved. */
  edited: boolean;
  /** Whether the person may save, save over, rename and delete: an owner or
   *  an admin, as the server holds. */
  canSave: boolean;
  /** Where a row's link opens. */
  hrefOf: (item: T) => string;
  /** What a click on a row does besides following its link. */
  onFollow?: (item: T, event: MouseEvent<HTMLAnchorElement>) => void;
  saveNew: (name: string) => Promise<ActionResult>;
  saveOver: (item: T) => Promise<ActionResult>;
  rename: (item: T, name: string) => Promise<ActionResult>;
  remove: (item: T) => Promise<ActionResult>;
  words: {
    /** What a new one holds, for the save dialog. */
    holds: string;
    /** What a rename keeps. */
    keeps: string;
    /** Who saves them, under the rows while the reader may not. */
    whoSaves: string;
    /** What deleting `item` does. */
    deleting: (item: T) => string;
  };
  /** Rows after the menu's own. */
  children?: ReactNode;
};

/** A change made and announced: the server's refusal as an error, else
 *  `done`. A change that moves the page does so in the action itself, whose
 *  redirect rejects the call and goes on to the router. */
export function useAnnounced(): (work: () => Promise<ActionResult>, done: string) => void {
  const [, start] = useTransition();
  return (work, done) =>
    start(async () => {
      const result = await work();
      if (!result.ok) {
        toast.error(result.error);
        return;
      }
      toast.success(done);
    });
}

export function SavedMenu<T extends Saved>({
  noun,
  saved,
  current,
  edited,
  canSave,
  hrefOf,
  onFollow,
  saveNew,
  saveOver,
  rename,
  remove,
  words,
  children,
}: Props<T>) {
  const announce = useAnnounced();
  const [naming, setNaming] = useState<"new" | "rename" | null>(null);
  const [deleting, setDeleting] = useState(false);
  const plural = PLURAL[noun];

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
            aria-label={current ? `Saved ${noun}: ${current.name}` : `Saved ${noun}s`}
          >
            <BookmarkIcon />
            <span className="max-w-40 truncate">{current ? current.name : plural}</span>
            {current && edited && (
              <span aria-hidden="true" title="Changed since it was saved">
                *
              </span>
            )}
            <Badge variant="secondary" className="px-1.5 tabular-nums">
              {saved?.length ?? 0}
            </Badge>
            <ChevronDownIcon className="text-muted-foreground" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-72">
          <DropdownMenuLabel>Saved {noun}s</DropdownMenuLabel>
          {saved === null ? (
            <DropdownMenuItem disabled>The {noun}s could not be read</DropdownMenuItem>
          ) : saved.length === 0 ? (
            <DropdownMenuItem disabled>No saved {noun}s yet</DropdownMenuItem>
          ) : (
            // Each a link, so one opens as any page does — in a new tab as
            // well — with the page kept where it is scrolled to.
            saved.map((item) => {
              const applied = item.id === current?.id;
              return (
                <DropdownMenuItem key={item.id} asChild>
                  <Link
                    href={hrefOf(item)}
                    scroll={false}
                    onClick={(event) => onFollow?.(item, event)}
                    aria-current={applied ? "true" : undefined}
                  >
                    <CheckIcon className={cn(!applied && "invisible")} aria-hidden="true" />
                    <span className="truncate">{item.name}</span>
                  </Link>
                </DropdownMenuItem>
              );
            })
          )}
          <DropdownMenuSeparator />
          <DropdownMenuItem disabled={!canSave || saved === null} onSelect={() => setNaming("new")}>
            <PlusIcon />
            Save as a new {noun}…
          </DropdownMenuItem>
          {current && (
            <>
              <DropdownMenuItem
                disabled={!canSave || !edited}
                onSelect={() => announce(() => saveOver(current), `Saved “${current.name}”`)}
              >
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
          {children}
          {!canSave && (
            <p className="px-2 py-1.5 text-xs text-muted-foreground">{words.whoSaves}</p>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      <NameDialog
        key={`${naming ?? "closed"}:${current?.id ?? ""}`}
        open={naming !== null}
        onOpenChange={(open) => !open && setNaming(null)}
        title={naming === "rename" ? `Rename the ${noun}` : `Save as a new ${noun}`}
        description={
          naming === "rename"
            ? `The ${noun} keeps ${words.keeps}.`
            : `Everyone on the project can open it from ${plural}: ${words.holds}, under a name.`
        }
        initial={naming === "rename" ? (current?.name ?? "") : ""}
        submitLabel={naming === "rename" ? "Rename" : `Save ${noun}`}
        onSubmit={async (name) => {
          const renamed = naming === "rename" ? current : undefined;
          const result = await (renamed ? rename(renamed, name) : saveNew(name));
          if (!result.ok) return result.error;
          toast.success(renamed ? `Renamed to “${name}”` : `Saved “${name}”`);
          return null;
        }}
      />

      {current && (
        <ConfirmDialog
          open={deleting}
          onOpenChange={setDeleting}
          title={`Delete “${current.name}”?`}
          description={words.deleting(current)}
          confirmLabel={`Delete ${noun}`}
          destructive
          onConfirm={() => announce(() => remove(current), `Deleted “${current.name}”`)}
        />
      )}
    </>
  );
}

/** A name asked for: saved, or the server's refusal shown beside the field —
 *  a name taken, a project at its hundred views. */
function NameDialog({
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
