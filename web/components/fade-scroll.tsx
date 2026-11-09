"use client";

import { useRef, type ReactNode } from "react";

import { useOverflow } from "@/lib/use-overflow";
import { cn } from "@/lib/utils";

/** Its one child scrolls with no scrollbar, the end with more fading
 *  (`data-scroll-x`/`data-scroll-y`, `[data-overflow]` in `globals.css`):
 *  sideways by default, or up and down with `axis="y"`. The child carries
 *  the attribute and its own `overflow-*-auto`. */
export function FadeScroll({
  className,
  axis = "x",
  children,
}: {
  className?: string;
  axis?: "x" | "y";
  children: ReactNode;
}) {
  const wrapper = useRef<HTMLDivElement>(null);
  const overflow = useOverflow(wrapper, axis);
  return (
    <div ref={wrapper} className={cn("relative min-w-0", className)} data-overflow={overflow || undefined}>
      {children}
    </div>
  );
}
