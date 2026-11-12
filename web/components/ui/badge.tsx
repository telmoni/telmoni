import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Slot } from "radix-ui"

import { cn } from "@/lib/utils"

// Keep badge density here so roles, counts and statuses share one shape: a
// 20px tag, the kbd's height, its text centred on its capitals (`trim-cap`).
const badgeVariants = cva(
  "inline-flex h-5 w-fit shrink-0 items-center justify-center gap-1 overflow-hidden rounded-sm border border-transparent px-2 text-xs font-normal whitespace-nowrap transition-[color,box-shadow] aria-invalid:border-destructive [&>svg]:pointer-events-none [&>svg]:size-icon",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground [a&]:hover:bg-primary/90",
        secondary:
          "bg-secondary text-secondary-foreground [a&]:hover:bg-secondary/90",
        // Statuses are tinted, not filled: the colour carries, the row stays calm.
        destructive: "bg-destructive/10 text-destructive",
        positive: "bg-brand-positive/10 text-brand-positive",
        warning: "bg-warning/15 text-warning",
        outline:
          "border-border text-foreground [a&]:hover:bg-accent [a&]:hover:text-accent-foreground",
        ghost: "[a&]:hover:bg-accent [a&]:hover:text-accent-foreground",
        link: "text-primary underline-offset-4 [a&]:hover:underline",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  }
)

function Badge({
  className,
  variant = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"span"> &
  VariantProps<typeof badgeVariants> & { asChild?: boolean }) {
  const Comp = asChild ? Slot.Root : "span"
  const { children, ...rest } = props
  // Text alone is trimmed to its capitals; anything else sits as it is.
  const plain = (c: unknown) => typeof c === "string" || typeof c === "number"
  const text = plain(children) || (Array.isArray(children) && children.every(plain))

  return (
    <Comp
      data-slot="badge"
      data-variant={variant}
      className={cn(badgeVariants({ variant }), className)}
      {...rest}
    >
      {text && !asChild ? <span className="trim-cap">{children}</span> : children}
    </Comp>
  )
}

export { Badge, badgeVariants }
