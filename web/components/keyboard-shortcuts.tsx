"use client";

import { useConsoleUi } from "@/components/console-ui-context";

import { useCallback, useEffect, useRef, useState } from "react";
import { usePathname, useRouter, useSelectedLayoutSegment } from "next/navigation";

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
  const pathname = usePathname();
  const root = useSelectedLayoutSegment();
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

  const pageActionRef = useRef(pageAction);
  const rootRef = useRef(root);
  const pathRef = useRef(pathname);
  useEffect(() => {
    pageActionRef.current = pageAction;
    rootRef.current = root;
    pathRef.current = pathname;
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
        const href = resolveSequence(k, pathRef.current || rootRef.current);
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
  }, [openSearch, openSheet, clearPending, router]);

  const visibleSequences = GO_SEQUENCES.filter(
    (s) =>
      s.label.toLowerCase().includes(filter.toLowerCase()) ||
      s.key.toLowerCase().includes(filter.toLowerCase()),
  );

  return (
    <Dialog
      open={sheetOpen}
      onOpenChange={(next) => {
        setSheetOpen(next);
        if (!next) setFilter("");
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Keyboard shortcuts</DialogTitle>
          <DialogDescription>
            Press these keys while not typing in an input field.
          </DialogDescription>
        </DialogHeader>

        <DialogBody className="space-y-4">
          <Input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter shortcuts…"
            aria-label="Filter shortcuts"
            className="h-8"
          />

          <div className="space-y-4 text-xs">
            <div>
              <div className="mb-2 font-medium text-muted-foreground">
                Navigation (press g, then key)
              </div>
              <div className="space-y-1">
                {visibleSequences.map((s) => (
                  <div
                    key={s.key}
                    className="flex items-center justify-between py-1"
                  >
                    <span>{s.label}</span>
                    <KbdGroup>
                      <Kbd>g</Kbd>
                      <Kbd>{s.key}</Kbd>
                    </KbdGroup>
                  </div>
                ))}
              </div>
            </div>

            <div>
              <div className="mb-2 font-medium text-muted-foreground">
                Actions
              </div>
              <div className="space-y-1">
                <div className="flex items-center justify-between py-1">
                  <span>Open search</span>
                  <KbdGroup>
                    <Kbd>{isMac ? "⌘" : "Ctrl"}</Kbd>
                    <Kbd>K</Kbd>
                  </KbdGroup>
                </div>
                <div className="flex items-center justify-between py-1">
                  <span>Open search (alternate)</span>
                  <Kbd>/</Kbd>
                </div>
                <div className="flex items-center justify-between py-1">
                  <span>Toggle assistant</span>
                  <KbdGroup>
                    <Kbd>{isMac ? "⌘" : "Ctrl"}</Kbd>
                    <Kbd>{AGENT_MODIFIER_KEY.toUpperCase()}</Kbd>
                  </KbdGroup>
                </div>
                <div className="flex items-center justify-between py-1">
                  <span>Primary page action</span>
                  <Kbd>{PAGE_ACTION_KEY}</Kbd>
                </div>
                <div className="flex items-center justify-between py-1">
                  <span>This sheet</span>
                  <Kbd>?</Kbd>
                </div>
              </div>
            </div>
          </div>
        </DialogBody>

        <DialogFooter className="flex-row items-center justify-between sm:justify-between border-t pt-3">
          <Label
            htmlFor="single-keys-toggle"
            className="text-xs text-muted-foreground cursor-pointer"
          >
            Enable single-key shortcuts
          </Label>
          <Switch
            id="single-keys-toggle"
            checked={!disabled}
            onCheckedChange={(enabled) => {
              setDisabled(!enabled);
              setSingleKeysDisabled(!enabled);
            }}
          />
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
