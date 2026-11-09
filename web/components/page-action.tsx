"use client";

import {
  cloneElement,
  createContext,
  isValidElement,
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
  variant,
  primary,
  children,
  ...props
}: ComponentProps<typeof Button> & { primary?: string }) {
  const ref = useRef<HTMLButtonElement>(null);
  const set = useContext(ActionSetCtx);
  const disabled = !!props.disabled;
  // The visual hint and announcement follow the same accessibility preference.
  const letterKeys = useLetterKeys();

  useEffect(() => {
    if (!primary || disabled || !set) return;
    const mine: PagePrimaryAction = {
      label: primary,
      // Never under a panel covering the page, which no pointer reaches either.
      run: () => {
        if (!ref.current?.closest("[inert]")) ref.current?.click();
      },
    };
    set(mine);
    return () => set((prev) => (prev === mine ? null : prev));
  }, [primary, disabled, set]);

  // The page's main action is its one filled button; the rest are outlined.
  const surface = variant ?? (primary ? "filled" : "outline");
  const buttonProps = {
    ref,
    variant: surface,
    size: "sm" as const,
    className: cn(surface !== "filled" && "text-muted-foreground hover:text-foreground", className),
    "aria-keyshortcuts": primary && !disabled && letterKeys ? PAGE_ACTION_KEY : undefined,
    ...props,
  };

  const hint = primary && !disabled && letterKeys
    ? <Kbd
        aria-hidden="true"
        className="size-5 shrink-0 p-0 in-data-[variant=filled]:bg-primary-foreground/15 in-data-[variant=filled]:text-primary-foreground"
      ><span className="trim-cap">{PAGE_ACTION_KEY.toUpperCase()}</span></Kbd>
    : null;
  const content = props.asChild && isValidElement<{ children?: ReactNode }>(children)
    ? cloneElement(children, {}, <>{children.props.children}{hint}</>)
    : <>{children}{hint}</>;

  return <Button {...buttonProps}>{content}</Button>;
}
