"use client";

import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ComponentProps,
  type Dispatch,
  type ReactNode,
  type SetStateAction,
} from "react";

import { Slot } from "radix-ui";

import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { PAGE_ACTION_KEY } from "@/lib/keys";
import { useLetterKeys } from "@/lib/use-accessibility";
import { cn } from "@/lib/utils";

export interface PagePrimaryAction {
  label: string;
  run: () => void;
}

type SetPrimaryAction = Dispatch<SetStateAction<PagePrimaryAction | null>>;

const ActionValueCtx = createContext<PagePrimaryAction | null>(null);
const ActionSetCtx = createContext<SetPrimaryAction | null>(null);

export function PagePrimaryActionProvider({
  children,
}: {
  children: ReactNode;
}) {
  const [action, setAction] = useState<PagePrimaryAction | null>(null);
  return (
    <ActionSetCtx.Provider value={setAction}>
      <ActionValueCtx.Provider value={action}>
        {children}
      </ActionValueCtx.Provider>
    </ActionSetCtx.Provider>
  );
}

export function usePagePrimaryAction(): PagePrimaryAction | null {
  return useContext(ActionValueCtx);
}

export function PageAction({
  className,
  variant = "outline",
  primary,
  children,
  ...props
}: ComponentProps<typeof Button> & { primary?: string }) {
  const ref = useRef<HTMLButtonElement>(null);
  const set = useContext(ActionSetCtx);
  const disabled = !!props.disabled;
  // Off in Accessibility, the key is not announced; its keycap hides by CSS.
  const letterKeys = useLetterKeys();

  useEffect(() => {
    if (!primary || disabled || !set) return;
    const mine: PagePrimaryAction = {
      label: primary,
      run: () => ref.current?.click(),
    };
    set(mine);
    return () => set((prev) => (prev === mine ? null : prev));
  }, [primary, disabled, set]);

  const keycap =
    primary && !disabled ? (
      <Kbd
        data-letter-key=""
        className={cn(
          variant === "default" && "bg-primary-foreground/20 text-primary-foreground",
        )}
      >
        {PAGE_ACTION_KEY.toUpperCase()}
      </Kbd>
    ) : null;
  const buttonProps = {
    ref,
    variant,
    size: "sm" as const,
    className: cn("gap-1.5 text-xs h-9 text-muted-foreground hover:text-foreground", className),
    "aria-keyshortcuts": primary && !disabled && letterKeys ? PAGE_ACTION_KEY : undefined,
    ...props,
  };

  // ⚠ A primary action that wraps a link (`asChild`) puts its keycap INSIDE
  // the link. The slot merges into one child element, so a keycap beside it
  // would be a second child it refuses; `Slottable` marks the link as the
  // element and hands it the keycap as a child of its own.
  if (primary && props.asChild) {
    return (
      <Button {...buttonProps}>
        <Slot.Slottable>{children}</Slot.Slottable>
        {keycap}
      </Button>
    );
  }
  return (
    <Button {...buttonProps}>
      {primary ? (
        <>
          {children}
          {keycap}
        </>
      ) : (
        children
      )}
    </Button>
  );
}
