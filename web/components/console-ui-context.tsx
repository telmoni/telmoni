"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { usePathname } from "next/navigation";
import {
  AGENT_TOGGLE_EVENT,
  SEARCH_OPEN_EVENT,
  SHORTCUTS_SHEET_EVENT,
} from "@/lib/keys";

/** What a create panel makes (`components/create-panels.tsx`). */
export type CreateKind = "project" | "organization";

/** The create panel open, and the control the focus goes back to as it
 *  shuts: the menu's trigger, where the menu row it was opened from is gone.
 *  `opened` counts the openings, so asking again draws the panel afresh. */
export interface Creating {
  kind: CreateKind;
  returnTo: HTMLElement | null;
  opened: number;
}

export interface ConsoleUiContextValue {
  searchOpen: boolean;
  setSearchOpen: (open: boolean) => void;
  openSearch: () => void;
  closeSearch: () => void;
  toggleSearch: () => void;

  agentOpen: boolean;
  setAgentOpen: (open: boolean) => void;
  openAgent: () => void;
  closeAgent: () => void;
  toggleAgent: () => void;

  shortcutsOpen: boolean;
  setShortcutsOpen: (open: boolean) => void;
  openShortcuts: () => void;
  closeShortcuts: () => void;
  toggleShortcuts: () => void;

  creating: Creating | null;
  openCreate: (kind: CreateKind, returnTo?: HTMLElement | null) => void;
  closeCreate: () => void;
}

const ConsoleUiContext = createContext<ConsoleUiContextValue | null>(null);

export function ConsoleUiProvider({ children }: { children: ReactNode }) {
  const [searchOpen, setSearchOpen] = useState(false);
  const [agentOpen, setAgentOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const [creating, setCreating] = useState<Creating | null>(null);

  // A create panel belongs to the page it was opened over, as a peeked trace
  // does to its list: another page shuts it.
  const pathname = usePathname();
  const [createdOver, setCreatedOver] = useState(pathname);
  if (createdOver !== pathname) {
    setCreatedOver(pathname);
    setCreating(null);
  }

  const openSearch = useCallback(() => setSearchOpen(true), []);
  const closeSearch = useCallback(() => setSearchOpen(false), []);
  const toggleSearch = useCallback(() => setSearchOpen((prev) => !prev), []);

  const openAgent = useCallback(() => setAgentOpen(true), []);
  const closeAgent = useCallback(() => setAgentOpen(false), []);
  const toggleAgent = useCallback(() => setAgentOpen((prev) => !prev), []);

  const openShortcuts = useCallback(() => setShortcutsOpen(true), []);
  const closeShortcuts = useCallback(() => setShortcutsOpen(false), []);
  const toggleShortcuts = useCallback(() => setShortcutsOpen((prev) => !prev), []);

  const openings = useRef(0);
  const openCreate = useCallback(
    (kind: CreateKind, returnTo: HTMLElement | null = null) =>
      setCreating({ kind, returnTo, opened: ++openings.current }),
    [],
  );
  const closeCreate = useCallback(() => setCreating(null), []);

  // Backward compatibility adapter for legacy DOM CustomEvents.
  useEffect(() => {
    const handleSearchOpen = () => openSearch();
    const handleAgentToggle = () => toggleAgent();
    const handleShortcutsSheet = () => openShortcuts();

    window.addEventListener(SEARCH_OPEN_EVENT, handleSearchOpen);
    window.addEventListener(AGENT_TOGGLE_EVENT, handleAgentToggle);
    window.addEventListener(SHORTCUTS_SHEET_EVENT, handleShortcutsSheet);

    return () => {
      window.removeEventListener(SEARCH_OPEN_EVENT, handleSearchOpen);
      window.removeEventListener(AGENT_TOGGLE_EVENT, handleAgentToggle);
      window.removeEventListener(SHORTCUTS_SHEET_EVENT, handleShortcutsSheet);
    };
  }, [openSearch, toggleAgent, openShortcuts]);

  return (
    <ConsoleUiContext.Provider
      value={{
        searchOpen,
        setSearchOpen,
        openSearch,
        closeSearch,
        toggleSearch,

        agentOpen,
        setAgentOpen,
        openAgent,
        closeAgent,
        toggleAgent,

        shortcutsOpen,
        setShortcutsOpen,
        openShortcuts,
        closeShortcuts,
        toggleShortcuts,

        creating,
        openCreate,
        closeCreate,
      }}
    >
      {children}
    </ConsoleUiContext.Provider>
  );
}

export function useConsoleUi(): ConsoleUiContextValue {
  const ctx = useContext(ConsoleUiContext);
  if (!ctx) {
    throw new Error("useConsoleUi must be used within a ConsoleUiProvider");
  }
  return ctx;
}
