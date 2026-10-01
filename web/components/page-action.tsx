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

import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { PAGE_ACTION_KEY } from "@/lib/keys";
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

  useEffect(() => {
    if (!primary || disabled || !set) return;
    const mine: PagePrimaryAction = {
      label: primary,
      run: () => ref.current?.click(),
    };
    set(mine);
    return () => set((prev) => (prev === mine ? null : prev));
  }, [primary, disabled, set]);

  return (
    <Button
      ref={ref}
      variant={variant}
      size="sm"
      className={cn(
        "gap-1.5 text-xs h-8 text-muted-foreground hover:text-foreground",
        className,
      )}
      aria-keyshortcuts={primary && !disabled ? PAGE_ACTION_KEY : undefined}
      {...props}
    >
      {primary ? (
        <>
          {children}
          {!disabled && (
            <Kbd
              className={cn(
                variant === "default" &&
                  "bg-primary-foreground/20 text-primary-foreground",
              )}
            >
              {PAGE_ACTION_KEY.toUpperCase()}
            </Kbd>
          )}
        </>
      ) : (
        children
      )}
    </Button>
  );
}
