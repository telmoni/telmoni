"use client";

import type { SearchIndex } from "@/app/(app)/search-actions";
import { useConsoleUi } from "@/components/console-ui-context";
import { useSearchIndex } from "@/components/search/use-search-index";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowUpRight,
  BookOpen,
  History,
  KeyRound,
  Search,
  User,
  Users,
  type LucideIcon,
} from "lucide-react";
import { usePathname, useRouter } from "next/navigation";

import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

import { consolePlace, projectAt } from "@/lib/console-nav";
import { projectPath } from "@/lib/slug";
import { useActiveOrganization, useProjects } from "@/lib/store";
import { SEARCH_MODIFIER_KEY } from "@/lib/keys";
import {
  GROUP_LABELS,
  groupItems,
  type SearchItem,
  type SearchKind,
} from "@/lib/search";
import { readRecent, type RecentVisit } from "@/lib/search-recent";
import { DOCS_URL } from "@/lib/site";
import { cn } from "@/lib/utils";

/**
 * The command palette.
 *
 * ⚠ **It has no trigger of its own — it is mounted once by the app layout and
 * opened by an event.** The trigger was a field in the header, which put a
 * second search box on the screen every time this opened and tied the panel's
 * size and position to whatever the header row happened to be doing. The way
 * in is the header's search button (`search-button.tsx`), ⌘K or `/`, and all
 * of them do the same thing: dispatch `SEARCH_OPEN_EVENT`.
 */
export function ConsoleSearch() {
  const router = useRouter();
  const projects = useProjects();
  // The slug of the organization those are the projects of.
  const organization = useActiveOrganization()?.slug ?? null;
  const projectId = projectAt(usePathname(), organization, projects)?.id;

  const { searchOpen: open, setSearchOpen: setOpen } = useConsoleUi();
  const [query, setQuery] = useState("");
  const [recent, setRecent] = useState<RecentVisit[]>([]);
  const { index, indexError, loading } = useSearchIndex(open, projectId);
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    const frame = requestAnimationFrame(() => {
      setRecent(readRecent());
      setActive(0);
      inputRef.current?.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [open]);

  // ⚠ **The chord lives with the thing it opens.** `keyboard-shortcuts.tsx`
  // returns early on any modifier, so a ⌘ chord cannot be added there without
  // unpicking that guard for every other key it protects. `nav-user.tsx` owns
  // ⌘⇧K the same way, and the two handlers cannot both fire: this one refuses
  // Shift and that one requires it.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() !== SEARCH_MODIFIER_KEY) return;
      if (!(e.ctrlKey || e.metaKey)) return;
      if (e.defaultPrevented || e.altKey || e.shiftKey) return;
      e.preventDefault();
      // A second press closes it, which is what every palette with this chord
      // does and what a person expects from a toggle.
      if (open) {
        setOpen(false);
        setQuery("");
        return;
      }
      setOpen(true);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [open, setOpen]);

  const placeOf = useCallback(
    (href: string) => {
      if (consolePlace(href)?.kind === "organization") return "Organization";
      return projectAt(href, organization, projects)?.name;
    },
    [organization, projects],
  );

  const items = useMemo<SearchItem[]>(() => {
    const out: SearchItem[] = [];
    for (const visit of recent) {
      out.push({
        id: `recent:${visit.href}`,
        kind: "recent",
        label: visit.label,
        hint: placeOf(visit.href) ?? visit.hint,
        href: visit.href,
      });
    }
    if (organization) {
      for (const project of projects) {
        out.push({
          id: `project:${project.id}`,
          kind: "project",
          label: project.name,
          hint: project.id,
          href: projectPath(organization, project.slug),
        });
      }
    }
    if (index) {
      for (const section of [index.keys, index.members]) {
        out.push(...section.items);
      }
    }
    return out;
  }, [recent, organization, projects, index, placeOf]);

  const docsItem = useMemo<SearchItem>(() => {
    return {
      id: "docs",
      kind: "docs",
      label: "Open the documentation",
      hint: "Docs",
      href: DOCS_URL,
      external: false,
    };
  }, []);

  const groups = useMemo(() => groupItems(items, query), [items, query]);

  const flat = useMemo(
    () => [...groups.flatMap((g) => g.items), docsItem],
    [groups, docsItem],
  );

  // ⚠ **`active` is an index into a list that changes under it.** The project's
  // index arrives mid-session and is dropped again on a project switch, so the
  // list can get SHORTER without a keystroke to reset the cursor — and a
  // cursor past the end selects nothing: no row is marked, and Enter opens
  // nothing, until you happen to press an arrow. Clamping keeps the highlight
  // where it was rather than throwing it back to the top.
  const activeIndex = flat.length === 0 ? 0 : Math.min(active, flat.length - 1);

  const go = useCallback(
    (item: SearchItem) => {
      setOpen(false);
      setQuery("");
      if (item.external) {
        window.open(item.href, "_blank", "noopener,noreferrer");
        return;
      }
      router.push(item.href);
    },
    [router, setOpen],
  );

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Escape") {
      setOpen(false);
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => (flat.length === 0 ? 0 : (i + 1) % flat.length));
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) =>
        flat.length === 0 ? 0 : (i - 1 + flat.length) % flat.length,
      );
      return;
    }
    if (e.key === "Enter") {
      const item = flat[activeIndex];
      if (item) {
        e.preventDefault();
        go(item);
      }
    }
  };

  const activeId = flat[activeIndex]?.id;

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setQuery("");
      }}
    >
      <DialogContent
        // ⚠ **Anchored near the top, not on the vertical centre.** The list
        // grows downward as results arrive; centred, every arriving row moved
        // the input box the person was typing in. A fixed top holds the input
        // still and lets the results fill in under it. `top-[12vh]` with
        // `translate-y-0` overrides only the vertical half of the dialog's
        // default centring — the horizontal half is what keeps it on the
        // screen's midline.
        className={cn(
          "top-[12vh] w-[min(40rem,calc(100vw-2rem))] max-w-none translate-y-0",
          "max-h-[70vh]",
        )}
      >
        {/* Named for a screen reader only: the field is the header here, and
            `sr-only` takes both out of the dialog's rows. */}
        <DialogTitle className="sr-only">Search</DialogTitle>
        <DialogDescription className="sr-only">
          Results filter as you type. Use the up and down arrow keys to move
          through them and Enter to open one.
        </DialogDescription>

        {/* The palette's header is its input row: the field and the close
            button on one row, inside the same 14px every modal keeps. */}
        <DialogHeader className="items-center gap-2.5 pb-3.5">
          <Search aria-hidden className="size-4 shrink-0 text-muted-foreground" />
          <input
            ref={inputRef}
            type="text"
            enterKeyHint="search"
            autoComplete="off"
            spellCheck={false}
            value={query}
            role="combobox"
            aria-expanded
            aria-controls={PANEL_ID}
            aria-autocomplete="list"
            aria-activedescendant={activeId ? rowId(activeId) : undefined}
            aria-label="Search this organization"
            placeholder={SEARCH_PROMPT}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
            }}
            onKeyDown={onKeyDown}
            className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
          />
        </DialogHeader>

        {/* The chassis's body, unpadded at the sides: a result row's
            highlight runs edge to edge, and each row keeps the 14px inset
            inside itself, the field's, as every modal's edge is. */}
        <DialogBody id={PANEL_ID} className="block p-0 py-1.5">
          {projectId && loading && (
            <p className="px-3.5 py-3 text-sm text-muted-foreground">
              Looking through this project&rsquo;s resources…
            </p>
          )}
          {indexError && (
            <p className="px-3.5 py-3 text-sm text-muted-foreground" role="status">
              {indexError}
            </p>
          )}
          {index && <IndexNotices index={index} />}

          {groups.length === 0 && query.trim() !== "" && (
            <p className="px-3.5 py-3 text-sm text-muted-foreground">
              Nothing here matches “{query.trim()}”.
            </p>
          )}
          <div role="listbox" aria-label="Search results">
            {groups.map((group) => (
              <div
                key={group.kind}
                role="group"
                aria-labelledby={headingId(group.kind)}
              >
                <GroupLabel id={headingId(group.kind)}>{group.label}</GroupLabel>
                {group.items.map((item) => (
                  <Row
                    key={item.id}
                    item={item}
                    activeId={activeId}
                    onPick={go}
                    onHover={() => setActive(flat.indexOf(item))}
                  />
                ))}
              </div>
            ))}
            <div role="group" aria-labelledby={headingId("docs")}>
              <GroupLabel id={headingId("docs")}>
                {GROUP_LABELS.docs}
              </GroupLabel>
              <Row
                item={docsItem}
                activeId={activeId}
                onPick={go}
                onHover={() => setActive(flat.length - 1)}
              />
            </div>
          </div>
        </DialogBody>

        {/* The legend. It is the only place the arrow keys are advertised, and
            it costs a row of chrome to say what would otherwise be found by
            accident. `aria-hidden`: the dialog's description already tells a
            screen reader the same thing, in a sentence. */}
        <DialogFooter
          aria-hidden
          className="justify-start text-xs text-muted-foreground"
        >
          <span>↑↓ to navigate</span>
          <span>·</span>
          <span>Enter to select</span>
          <span>·</span>
          <span>Esc to close</span>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function GroupLabel({ id, children }: { id: string; children: string }) {
  return (
    <p
      id={id}
      className="px-3.5 pt-3 pb-1 text-xs font-medium tracking-wide text-muted-foreground uppercase"
    >
      {children}
    </p>
  );
}

// One glyph per kind, so a row says what it is before its label is read. The
// group heading says it too, but a heading scrolls out of view and the row
// does not.
const KIND_ICON: Record<SearchKind, LucideIcon> = {
  recent: History,
  project: Users,
  key: KeyRound,
  member: User,
  docs: BookOpen,
};

function Row({
  item,
  activeId,
  onPick,
  onHover,
}: {
  item: SearchItem;
  activeId: string | undefined;
  onPick: (item: SearchItem) => void;
  onHover: () => void;
}) {
  const Icon = KIND_ICON[item.kind];
  return (
    <button
      type="button"
      id={rowId(item.id)}
      role="option"
      aria-selected={item.id === activeId}
      tabIndex={-1}
      onMouseDown={(e) => {
        e.preventDefault();
        onPick(item);
      }}
      onMouseEnter={onHover}
      className={cn(
        "flex w-full items-center gap-2.5 px-3.5 py-2 text-left",
        item.id === activeId && "bg-accent",
      )}
    >
      <Icon aria-hidden className="size-4 shrink-0 text-muted-foreground" />
      <span className="grid min-w-0 flex-1 gap-0.5">
        <span className="truncate text-sm">{item.label}</span>
        {item.hint && (
          <span className="truncate text-xs text-muted-foreground">
            {item.hint}
          </span>
        )}
      </span>
      {item.external && (
        <ArrowUpRight
          aria-label="Opens in a new tab"
          className="size-3.5 shrink-0 text-muted-foreground"
        />
      )}
    </button>
  );
}

// The palette's placeholder, and the one place with room for the long form.
// No key cap in it: the chord OPENS this, so naming it to somebody already
// typing here is an instruction they have carried out. The header button's
// tooltip and the shortcuts sheet are what name the chord.
const SEARCH_PROMPT = "Search for resources, docs, projects, and more";

const PANEL_ID = "console-search-panel";

function headingId(kind: string): string {
  return `console-search-group-${kind}`;
}

function rowId(itemId: string): string {
  return `console-search-row-${itemId.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
}

function IndexNotices({ index }: { index: SearchIndex }) {
  const refused = SECTIONS.filter(([, pick]) => pick(index).forbidden);
  const broken = SECTIONS.filter(([, pick]) => pick(index).unavailable);
  if (refused.length === 0 && broken.length === 0) return null;
  return (
    <div className="border-b border-border px-3.5 py-2">
      {refused.length > 0 && (
        <p className="text-xs text-muted-foreground">
          Not searched — your role on this project cannot list{" "}
          {readable(refused.map(([name]) => name))}.
        </p>
      )}
      {broken.length > 0 && (
        <p className="text-xs text-muted-foreground">
          Couldn&rsquo;t read {readable(broken.map(([name]) => name))} just now.
        </p>
      )}
    </div>
  );
}

const SECTIONS: [string, (i: SearchIndex) => SearchIndex[keyof SearchIndex]][] = [
  ["API keys", (i) => i.keys],
  ["members", (i) => i.members],
];

function readable(names: string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}
