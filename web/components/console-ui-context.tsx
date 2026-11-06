"use client";

import {
  createContext,
  useContext,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type ReactNode,
  type SetStateAction,
} from "react";
import { usePathname, useRouter } from "next/navigation";
import { toast } from "sonner";

import { useProjectCeiling } from "@/lib/store";

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

/** The console's windows, each a flag and its setter: the search palette,
 *  the agent's window, the shortcuts sheet and the create panel. */
export interface ConsoleUiContextValue {
  searchOpen: boolean;
  setSearchOpen: Dispatch<SetStateAction<boolean>>;
  agentOpen: boolean;
  setAgentOpen: Dispatch<SetStateAction<boolean>>;
  shortcutsOpen: boolean;
  setShortcutsOpen: Dispatch<SetStateAction<boolean>>;
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

  // At the plan's ceiling a New project goes where the ceiling is lifted,
  // its sentence said, rather than to a panel whose name would be refused.
  const ceiling = useProjectCeiling();
  const router = useRouter();

  const openings = useRef(0);
  // One value until a flag moves: the provider renders at every navigation,
  // and its consumers need not.
  const value = useMemo<ConsoleUiContextValue>(
    () => ({
      searchOpen,
      setSearchOpen,
      agentOpen,
      setAgentOpen,
      shortcutsOpen,
      setShortcutsOpen,
      creating,
      openCreate: (kind, returnTo = null) => {
        if (kind === "project" && ceiling?.reached) {
          toast.info(ceiling.detail);
          if (ceiling.page) router.push(ceiling.page);
          return;
        }
        setCreating({ kind, returnTo, opened: ++openings.current });
      },
      closeCreate: () => setCreating(null),
    }),
    [searchOpen, agentOpen, shortcutsOpen, creating, ceiling, router],
  );

  return <ConsoleUiContext value={value}>{children}</ConsoleUiContext>;
}

export function useConsoleUi(): ConsoleUiContextValue {
  const ctx = useContext(ConsoleUiContext);
  if (!ctx) {
    throw new Error("useConsoleUi must be used within a ConsoleUiProvider");
  }
  return ctx;
}
