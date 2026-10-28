"use client";

import { useConsoleUi } from "@/components/console-ui-context";

import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import Link from "next/link";
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
import { Input } from "@/components/ui/input";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { usePagePrimaryAction } from "@/components/page-action";
import { pageCovered, usePageCovered } from "@/lib/page-cover";
import { useActiveOrganization, useFlags } from "@/lib/store";
import { letterKeysOn, useLetterKeys } from "@/lib/use-accessibility";
import { pageSteps, usePageSteps } from "@/lib/use-page-steps";
import { useIsMac } from "@/lib/use-platform";
import {
  AGENT_MODIFIER_KEY,
  goSequences,
  inFront,
  NEXT_KEY,
  OPEN_KEY,
  PAGE_ACTION_KEY,
  PREVIOUS_KEY,
  pressedKey,
  resolveSequence,
  SEQUENCE_TIMEOUT_MS,
  SEARCH_KEY,
  typedInto,
} from "@/lib/keys";

export function KeyboardShortcuts() {
  const router = useRouter();
  // Where you are: the path names the organization and the project, and
  // `resolveSequence` reads it through `consolePlace`, the scheme's one home.
  // The organization the console stands in is for `g O` from Account, whose
  // path names none.
  const pathname = usePathname();
  const organization = useActiveOrganization()?.slug ?? null;
  const flags = useFlags();
  const pageAction = usePagePrimaryAction();
  const steps = usePageSteps();
  const isMac = useIsMac();

  const { shortcutsOpen: sheetOpen, setShortcutsOpen: setSheetOpen, openSearch } = useConsoleUi();
  const [filter, setFilter] = useState("");
  // Letter-key shortcuts are an Accessibility setting (`lib/accessibility.ts`):
  // off, the handler below ignores bare keys and the sheet lists only chords.
  const letterKeys = useLetterKeys();
  const openSheet = useCallback(() => setSheetOpen(true), [setSheetOpen]);

  const pendingG = useRef(false);
  const pendingTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const clearPending = useCallback(() => {
    pendingG.current = false;
    if (pendingTimer.current) {
      clearTimeout(pendingTimer.current);
      pendingTimer.current = null;
    }
  }, []);

  // The keydown handler needs the page's primary action, which changes as you
  // navigate. Reading it from a ref lets the listener below bind once; naming
  // it as a dependency would re-add a document-level listener on every render.
  // Ref-sync effect with no dependency array, as in realtime-listener.tsx.
  const pageActionRef = useRef(pageAction);
  // Where you ARE decides where `g k` goes, since every sequence but `g O`
  // and `g n` is a row of the rail you stand in. Through a ref, as the page
  // action: the path as a dependency would re-bind the listener each move.
  const placeRef = useRef({ pathname, organization, flags });
  useEffect(() => {
    pageActionRef.current = pageAction;
    placeRef.current = { pathname, organization, flags };
  });

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey || typedInto(e)) return;
      if (inFront()) {
        clearPending();
        return;
      }
      if (!letterKeysOn()) return;
      const key = pressedKey(e);
      if (key.length !== 1) return;
      if (e.repeat) return;
      const k = e.shiftKey ? key : key.toLowerCase();

      if (pendingG.current) {
        clearPending();
        // A panel over the page whole holds the rail's sequences off, as it
        // does the page's action: the key after `g` goes nowhere.
        if (pageCovered()) return;
        const href = resolveSequence(
          k,
          placeRef.current.pathname,
          placeRef.current.organization,
          placeRef.current.flags,
        );
        if (href) {
          e.preventDefault();
          router.push(href);
        }
        return;
      }
      if (k === "g") {
        e.preventDefault();
        pendingG.current = true;
        pendingTimer.current = setTimeout(clearPending, SEQUENCE_TIMEOUT_MS);
        return;
      }
      const action = pageActionRef.current;
      if (k === PAGE_ACTION_KEY && action) {
        e.preventDefault();
        action.run();
        return;
      }
      const step = pageSteps();
      if (step && (k === NEXT_KEY || k === PREVIOUS_KEY)) {
        e.preventDefault();
        (k === NEXT_KEY ? step.next : step.previous)();
        return;
      }
      if (step?.open && k === OPEN_KEY) {
        e.preventDefault();
        step.open();
        return;
      }
      // The sheet is the `?` the keyboard types and Search the `/`, whatever
      // Shift took to type them: on a German, Nordic, Spanish, Italian or
      // French keyboard `/` is itself a shifted key, and reading Shift as the
      // sheet's left those people no `/` for Search at all.
      if (k === "?") {
        e.preventDefault();
        openSheet();
        return;
      }
      if (k === SEARCH_KEY) {
        e.preventDefault();
        openSearch();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [router, openSheet, clearPending, openSearch]);


  const q = filter.trim().toLowerCase();
  const modifier = isMac ? "\u2318" : "Ctrl";
  const covered = usePageCovered();
  // The rail you stand in, straight from the path rather than the ref, since
  // the sheet re-renders: it lists exactly the rows a sequence reaches here.
  const rows = letterKeys && !covered
    ? goSequences(pathname, organization, flags).filter((s) => s.label.toLowerCase().includes(q))
    : [];
  // One row per thing you can do, with every chord that does it, so Search is
  // listed once under both of its keys.
  // With letter keys off, only the chords held with a modifier are listed:
  // the sheet shows what works where you are, never a key that does nothing.
  const extras = [
    ...(pageAction && letterKeys && !covered
      ? [{ label: pageAction.label, chords: [[PAGE_ACTION_KEY.toUpperCase()]] }]
      : []),
    ...(steps && letterKeys
      ? [
          { label: `Next ${steps.noun}`, chords: [[NEXT_KEY.toUpperCase()]] },
          { label: `Previous ${steps.noun}`, chords: [[PREVIOUS_KEY.toUpperCase()]] },
          ...(steps.open
            ? [{ label: `Open or shut the ${steps.noun}`, chords: [[OPEN_KEY.toUpperCase()]] }]
            : []),
        ]
      : []),
    { label: "Search", chords: letterKeys ? [[modifier, "K"], [SEARCH_KEY]] : [[modifier, "K"]] },
    { label: "Ask the agent", chords: [[modifier, AGENT_MODIFIER_KEY.toUpperCase()]] },
    { label: "Open the account menu", chords: [[modifier, "Shift", "K"]] },
    // Shown as the `?` it is, since the keys that type one differ from one
    // keyboard to the next: Shift and `/` on a US one, Shift and `ß` on a
    // German one.
    ...(letterKeys ? [{ label: "This sheet", chords: [["?"]] }] : []),
  ].filter((c) => c.label.toLowerCase().includes(q));

  return (
    <Dialog
      open={sheetOpen}
      onOpenChange={(next) => {
        setSheetOpen(next);
        if (!next) setFilter("");
      }}
    >
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Keyboard shortcuts</DialogTitle>
        </DialogHeader>
        {/* A column rather than the body's own scroll, so the filter holds
            still at the top and only the list under it scrolls. */}
        <DialogBody className="flex flex-col overflow-hidden">
          <DialogDescription>
            Go anywhere without the mouse. Everything here is also a link or
            a button in the console.
          </DialogDescription>
          <Input
            aria-label="Filter shortcuts"
            placeholder="Filter shortcuts…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
          {/* `pr-1`: the keys stand off the scrollbar by the gap between two
              keys, instead of touching it. */}
          <div className="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto pr-1">
            {rows.map((s) => (
              <div
                key={s.key}
                className="flex h-8 items-center justify-between text-sm"
              >
                <span>{s.label}</span>
                <KbdGroup>
                  <Kbd>G</Kbd>
                  <Kbd>{/^[A-Z]$/.test(s.key) ? `⇧ ${s.key}` : s.key.toUpperCase()}</Kbd>
                </KbdGroup>
              </div>
            ))}
            {extras.map((c) => (
              <div key={c.label} className="flex h-8 items-center justify-between text-sm">
                <span>{c.label}</span>
                <span className="flex items-center gap-1.5">
                  {c.chords.map((chord, i) => (
                    <Fragment key={chord.join("+")}>
                      {i > 0 && <span className="text-xs text-muted-foreground">or</span>}
                      <KbdGroup>
                        {chord.map((k) => (
                          <Kbd key={k}>{k}</Kbd>
                        ))}
                      </KbdGroup>
                    </Fragment>
                  ))}
                </span>
              </div>
            ))}
            {rows.length === 0 && extras.length === 0 && (
              <p className="py-2 text-sm text-muted-foreground">
                No shortcut matches.
              </p>
            )}
          </div>
        </DialogBody>
        {/* The sheet's one control is its footer, where every modal keeps
            what it lets you act on. */}
        {/* The setting lives in Accessibility; the sheet says which way it
            stands and leads there. */}
        {/* `py-2.5`: the note is text, whose line spacing sets its words about
            4px inside its box above and below, so the box sits 10px in for
            the words to read 14px from the list and from the edge, as the
            sides do. */}
        <DialogFooter className="justify-start py-2.5 text-xs text-muted-foreground">
          <p>
            {letterKeys
              ? "Use voice control or a screen reader? Letter-key shortcuts can be turned off in "
              : "Letter-key shortcuts are off, so only the shortcuts above work. Turn them on in "}
            <Link
              href="/account/accessibility"
              onClick={() => setSheetOpen(false)}
              className="underline underline-offset-2 hover:text-foreground"
            >
              Accessibility
            </Link>
            .
          </p>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
