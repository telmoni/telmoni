import * as React from "react"

import { cn } from "@/lib/utils"

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "h-control w-full min-w-0 flex items-center rounded-md border border-input bg-transparent px-gutter py-1 text-sm transition-[color,box-shadow]",
        "selection:bg-primary selection:text-primary-foreground",
        "file:inline-flex file:h-5 file:border-0 file:bg-transparent file:text-xs file:font-medium file:text-foreground",
        "placeholder:text-muted-foreground",
        "disabled:cursor-not-allowed disabled:opacity-50",
        // Focus is the global field rule's (`globals.css`): the border alone.
        "aria-invalid:border-destructive",
        // Outlined in the dark theme too, as the select's trigger is: a
        // filled field with no edge reads as a panel, not a place to type.
        "dark:border-border dark:bg-input/30",
        className
      )}
      {...props}
    />
  )
}

export { Input }
