import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Slot } from "radix-ui"

import { cn } from "@/lib/utils"

const buttonVariants = cva(
  "inline-flex shrink-0 cursor-pointer items-center justify-center gap-gutter rounded-md bg-transparent text-sm font-medium whitespace-nowrap transition-[color,background-color,border-color,box-shadow] disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg]:size-icon",
  {
    variants: {
      variant: {
        filled: "border border-primary bg-primary text-primary-foreground hover:bg-primary/90",
        destructive:
          "border border-destructive bg-destructive text-destructive-foreground hover:bg-destructive/90 dark:border-destructive-fill dark:bg-destructive-fill dark:hover:bg-destructive-fill/90",
        outline:
          "border border-border hover:border-foreground/40",
        ghost:
          "hover:bg-accent hover:text-accent-foreground",
        link: "text-foreground underline underline-offset-4 hover:no-underline",
      },
      // The shared control height keeps actions aligned with fields and links.
      size: {
        default: "h-control px-gutter text-sm",
        sm: "h-control px-gutter text-sm",
        icon: "size-control",
      },
    },
    defaultVariants: {
      variant: "outline",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "outline",
  size = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot.Root : "button"

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Button, buttonVariants }
