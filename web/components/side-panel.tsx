"use client";

import {
  useLayoutEffect,
  useState,
  type ComponentProps,
  type CSSProperties,
  type ReactNode,
  type RefObject,
} from "react";
import { XIcon, type LucideIcon } from "lucide-react";

import { useConsoleShell } from "@/components/console-shell";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Label } from "@/components/ui/label";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { useNonModal } from "@/components/use-non-modal";
import { cn } from "@/lib/utils";

// The console's side panel: over the right
// side of the content area — under the header, beside the rail, neither
// covered — and non-modal, so the page beside it keeps its pointer and its
// keys; or `full`, over the area whole. The trace list's peek stands in one
// (`components/traces/peek.tsx`), and New project and New organization
// (`create-panels.tsx`) in a full one.

/** Whether the panel stands as wide as the content area it is in: on a
 *  phone, beside a page too narrow for its own width, or dragged all the
 *  way — measured, since each of those is the window's or the hand's. */
function useCoversArea(panel: RefObject<HTMLDivElement | null>): boolean {
  const [covers, setCovers] = useState(false);
  useLayoutEffect(() => {
    const element = panel.current;
    const area = element?.parentElement;
    if (!element || !area || typeof ResizeObserver === "undefined") return;
    // An area not yet laid out measures nothing, and covers nothing either.
    const measure = () => {
      const whole = area.getBoundingClientRect().width;
      setCovers(whole > 0 && element.getBoundingClientRect().width >= whole - 1);
    };
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    observer.observe(area);
    measure();
    return () => observer.disconnect();
  }, [panel]);
  return covers;
}

/** One of the header's buttons: an icon, muted as the console header's are,
 *  named for a screen reader and, under the pointer, in a tooltip with its
 *  letter key where it has one. A step past the end of a list is
 *  `unavailable` rather than disabled, so the focus stays on it. */
export function PanelAction({
  label,
  letterKey,
  unavailable,
  onClick,
  children,
  className,
  ...props
}: {
  label: string;
  letterKey?: string;
  unavailable?: boolean;
  children: ReactNode;
} & Omit<ComponentProps<typeof Button>, "aria-label" | "aria-disabled">) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          aria-label={label}
          aria-disabled={unavailable || undefined}
          onClick={unavailable ? undefined : onClick}
          className={cn(
            "text-muted-foreground hover:text-foreground",
            "aria-disabled:cursor-not-allowed aria-disabled:opacity-50",
            className,
          )}
          {...props}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent>
        {label}
        {letterKey && (
          <Kbd data-letter-key="" className="ml-1.5">
            {letterKey.toUpperCase()}
          </Kbd>
        )}
      </TooltipContent>
    </Tooltip>
  );
}

export function SidePanel({
  panel,
  titleId,
  onClose,
  full = false,
  width = null,
  initialFocus,
  returnFocus,
  children,
}: {
  /** The panel's element, which the panel's own content reads too. */
  panel: RefObject<HTMLDivElement | null>;
  titleId: string;
  /** Escape pressed inside it. */
  onClose: () => void;
  /** Whether it fills the content area. */
  full?: boolean;
  /** A width the person dragged it to, in pixels; `null` for its own. */
  width?: number | null;
  /** What takes the focus as it opens; the panel itself if none. */
  initialFocus?: RefObject<HTMLElement | null>;
  /** Where the focus goes as it shuts with the focus inside; the element
   *  that had it before it opened if this answers none. */
  returnFocus?: () => HTMLElement | null | undefined;
  children: ReactNode;
}) {
  const { cover } = useConsoleShell();
  const coversArea = useCoversArea(panel);
  const covering = full || coversArea;

  // Over the page whole, the page is out of reach of the keyboard and of a
  // screen reader, as it is of the eye. Declared before the focus's effect,
  // so a panel shutting uncovers the page before handing the focus back.
  useLayoutEffect(() => (covering ? cover() : undefined), [covering, cover]);

  useNonModal(panel, { onClose, initialFocus, returnFocus });

  return (
    <div
      ref={panel}
      data-slot="side-panel"
      role="dialog"
      // Non-modal, and said so: the page beside it keeps its pointer and its
      // keys, which the letter shortcuts read this to know (`MODAL_SELECTOR`,
      // `lib/keys.ts`).
      aria-modal="false"
      aria-labelledby={titleId}
      tabIndex={-1}
      data-mode={full ? "full" : "side"}
      style={width === null ? undefined : ({ "--panel-width": `${width}px` } as CSSProperties)}
      className={cn(
        "absolute inset-y-0 right-0 z-30 flex flex-col overflow-hidden bg-background outline-none",
        "animate-in duration-200 fade-in-0 slide-in-from-right-4",
        full
          ? "left-0"
          : cn(
              "left-0 md:left-auto md:max-w-full md:border-l md:border-border md:shadow-drawer",
              width !== null ? "md:w-(--panel-width)" : "md:w-[max(40rem,70%)]",
            ),
      )}
    >
      <TooltipProvider>{children}</TooltipProvider>
    </div>
  );
}

/** The panel's head: what it shows, named beside its glyph, its buttons ending on Close, a
 *  line summing it up, and a rule inset as the rows' rules are. */
export function SidePanelHeader({
  icon: Icon,
  titleId,
  title,
  beside,
  actions,
  onClose,
  flush = false,
  children,
}: {
  icon: LucideIcon;
  titleId: string;
  title: ReactNode;
  /** After the title: an id, say, and its copy button. */
  beside?: ReactNode;
  /** Before Close. */
  actions?: ReactNode;
  onClose: () => void;
  /** Its rule edge to edge, over panes whose own rules run so. */
  flush?: boolean;
  /** The line under the title. */
  children?: ReactNode;
}) {
  return (
    <div className={cn("shrink-0 px-3.5", flush && "border-b border-border")}>
      <div className={cn("grid gap-1.5 py-3.5", !flush && "border-b border-border")}>
        <div className="flex min-h-9 min-w-0 items-center gap-2">
          <Icon className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
          <h2 id={titleId} className="min-w-0 truncate text-base font-medium">
            {title}
          </h2>
          {beside}
          <div className="ml-auto flex shrink-0 items-center gap-0.5">
            {actions}
            <PanelAction label="Close" onClick={onClose}>
              <XIcon />
            </PanelAction>
          </div>
        </div>
        {children}
      </div>
    </div>
  );
}

/** The panel's foot, its buttons at the right: as tall as the rail's foot
 *  and the header, so its rule runs on from the rail's. */
export function SidePanelFooter({ children }: { children: ReactNode }) {
  return (
    <div className="flex h-15 shrink-0 items-center justify-end gap-3 border-t border-border px-3.5">
      {children}
    </div>
  );
}

/** One line of what the panel holds, a name beside its value as a span's
 *  details read (`trace-view.tsx`); stacked on a phone. With `htmlFor`, the
 *  name labels the field it stands beside. Its 14px above and below are the
 *  body's only vertical inset, so every name stands as far below its rule;
 *  the rule is drawn above it, so a row a slot wraps alone is still ruled
 *  off from the next. */
export function SidePanelRow({
  label,
  htmlFor,
  children,
}: {
  label: string;
  htmlFor?: string;
  children: ReactNode;
}) {
  return (
    <div className="grid gap-1.5 border-t border-border py-3.5 first:border-t-0 sm:grid-cols-[minmax(0,9rem)_minmax(0,1fr)] sm:gap-3">
      {htmlFor ? (
        <Label htmlFor={htmlFor} className="font-normal text-muted-foreground sm:h-9">
          {label}
        </Label>
      ) : (
        <span className="text-sm text-muted-foreground">{label}</span>
      )}
      <div className="grid min-w-0 content-start gap-1.5 text-sm">{children}</div>
    </div>
  );
}
