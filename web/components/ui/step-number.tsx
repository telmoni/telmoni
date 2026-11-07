import type { ComponentProps } from "react";

import { cn } from "@/lib/utils";

/** A number in a square, centred on its digits (`trim-cap`); an icon in its place sits as it is. */
export function StepNumber({ children, className, ...props }: ComponentProps<"span">) {
  const text = typeof children === "number" || typeof children === "string";
  return (
    <span
      className={cn(
        "flex size-control shrink-0 items-center justify-center rounded-md border border-border text-sm tabular-nums",
        className,
      )}
      {...props}
    >
      {text ? <span className="trim-cap">{children}</span> : children}
    </span>
  );
}
