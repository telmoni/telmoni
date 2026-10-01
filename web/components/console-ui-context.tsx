"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import {
  AGENT_TOGGLE_EVENT,
  SEARCH_OPEN_EVENT,
  SHORTCUTS_SHEET_EVENT,
} from "@/lib/keys";

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
}

const ConsoleUiContext = createContext<ConsoleUiContextValue | null>(null);

export function ConsoleUiProvider({ children }: { children: ReactNode }) {
  const [searchOpen, setSearchOpen] = useState(false);
  const [agentOpen, setAgentOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);

  const openSearch = useCallback(() => setSearchOpen(true), []);
  const closeSearch = useCallback(() => setSearchOpen(false), []);
  const toggleSearch = useCallback(() => setSearchOpen((prev) => !prev), []);

  const openAgent = useCallback(() => setAgentOpen(true), []);
  const closeAgent = useCallback(() => setAgentOpen(false), []);
  const toggleAgent = useCallback(() => setAgentOpen((prev) => !prev), []);

  const openShortcuts = useCallback(() => setShortcutsOpen(true), []);
  const closeShortcuts = useCallback(() => setShortcutsOpen(false), []);
  const toggleShortcuts = useCallback(() => setShortcutsOpen((prev) => !prev), []);

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
