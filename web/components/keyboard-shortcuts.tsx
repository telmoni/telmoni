"use client";

import { useConsoleUi } from "@/components/console-ui-context";

import { useCallback, useEffect, useRef, useState } from "react";
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
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { usePagePrimaryAction } from "@/components/page-action";
import { useActiveOrganization } from "@/lib/store";
import { useIsMac } from "@/lib/use-platform";
import {
  AGENT_MODIFIER_KEY,
  GO_SEQUENCES,
  PAGE_ACTION_KEY,
  resolveSequence,
  SEQUENCE_TIMEOUT_MS,
  SEARCH_KEY,
  SHORTCUTS_DISABLED_KEY,
} from "@/lib/keys";

let inMemoryDisabled = false;

function singleKeysDisabled() {
  try {
    inMemoryDisabled =
      window.localStorage.getItem(SHORTCUTS_DISABLED_KEY) === "1";
    return inMemoryDisabled;
  } catch {
    return inMemoryDisabled;
  }
}

function setSingleKeysDisabled(next: boolean) {
  inMemoryDisabled = next;
  try {
    if (next) window.localStorage.setItem(SHORTCUTS_DISABLED_KEY, "1");
    else window.localStorage.removeItem(SHORTCUTS_DISABLED_KEY);
  } catch {
  }
}

export function KeyboardShortcuts() {
  const router = useRouter();
  // Where you are: the path names the organization and the project, and
  // `resolveSequence` reads it through `consolePlace`, the scheme's one home.
  // The organization the console stands in is for `g O` from Account, whose
  // path names none.
  const pathname = usePathname();
  const organization = useActiveOrganization()?.slug ?? null;
  const pageAction = usePagePrimaryAction();
  const isMac = useIsMac();

  const { shortcutsOpen: sheetOpen, setShortcutsOpen: setSheetOpen, openSearch } = useConsoleUi();
  const [filter, setFilter] = useState("");
  const [disabled, setDisabled] = useState(false);
  const openSheet = useCallback(() => {
    setDisabled(singleKeysDisabled());
    setSheetOpen(true);
  }, [setSheetOpen]);

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
  // Six of the seven sequences are project-relative, so where you ARE decides
  // where `g k` goes. Through a ref for the same reason as the page action:
  // naming the path as a dependency re-binds a document listener on every
  // navigation, and this one has to survive them.
  const placeRef = useRef({ pathname, organization });
  useEffect(() => {
    pageActionRef.current = pageAction;
    placeRef.current = { pathname, organization };
  });

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey) return;
      const target = e.target instanceof HTMLElement ? e.target : null;
      if (
        target?.closest(
          'input, textarea, select, [contenteditable="true"], [role="combobox"], [role="listbox"]',
        )
      )
        return;
      if (
        document.querySelector(
          '[role="dialog"], [role="alertdialog"], [role="menu"]',
        )
      ) {
        clearPending();
        return;
      }
      if (singleKeysDisabled()) return;
      if (e.key.length > 1) return;
      if (e.repeat) return;
      const k = e.shiftKey ? e.key : e.key.toLowerCase();

      if (pendingG.current) {
        clearPending();
        const href = resolveSequence(
          k,
          placeRef.current.pathname,
          placeRef.current.organization,
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
      if (k === SEARCH_KEY) {
        e.preventDefault();
        openSearch();
        return;
      }
      if (k === "?") {
        e.preventDefault();
        openSheet();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [router, openSheet, clearPending, openSearch]);


  const q = filter.trim().toLowerCase();
  const modifier = isMac ? "\u2318" : "Ctrl";
  // ⚠ **The same missing argument as the handler, and here it silently
  // widened the sheet instead of breaking a key.** This filter is meant to
  // hide the sequences that have no destination from where you stand; with
  // nowhere to stand every one of them resolved to something, so it hid
  // nothing and the sheet promised `g k` on the organization pages, where it
  // does not go. Straight from the path rather than the ref: the sheet
  // re-renders.
  const rows = GO_SEQUENCES.filter(
    (s) =>
      resolveSequence(s.key, pathname, organization) !== null &&
      s.label.toLowerCase().includes(q),
  );
  const extras = [
    ...(pageAction
      ? [{ label: pageAction.label, keys: [PAGE_ACTION_KEY.toUpperCase()] }]
      : []),
    { label: "Search", keys: [modifier, "K"] },
    { label: "Search", keys: [SEARCH_KEY] },
    { label: "Ask the agent", keys: [modifier, AGENT_MODIFIER_KEY.toUpperCase()] },
    { label: "Open the account menu", keys: [modifier, "Shift", "K"] },
    { label: "This sheet", keys: ["?"] },
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
          <div className="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto">
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
              <div
                // The label alone repeats: Search is listed under both of
                // its keys.
                key={`${c.label}:${c.keys.join("+")}`}
                className="flex h-8 items-center justify-between text-sm"
              >
                <span>{c.label}</span>
                <KbdGroup>
                  {c.keys.map((k) => (
                    <Kbd key={k}>{k}</Kbd>
                  ))}
                </KbdGroup>
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
        <DialogFooter className="justify-between">
          <Label htmlFor="single-key-shortcuts" className="font-normal">
            Single-key shortcuts
          </Label>
          <Switch
            id="single-key-shortcuts"
            checked={!disabled}
            onCheckedChange={(on) => {
              setDisabled(!on);
              setSingleKeysDisabled(!on);
            }}
          />
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
