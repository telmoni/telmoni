import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Slot } from "radix-ui"

import { cn } from "@/lib/utils"

const buttonVariants = cva(
  "inline-flex shrink-0 cursor-pointer items-center justify-center gap-1.5 rounded-md bg-transparent text-sm font-medium whitespace-nowrap transition-[color,background-color,border-color,box-shadow] outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg]:size-4",
  {
    variants: {
      variant: {
        default: "border border-border hover:border-foreground/40",
        filled: "border border-primary bg-primary text-primary-foreground hover:bg-primary/90",
        destructive:
          "border border-destructive bg-destructive text-destructive-foreground hover:bg-destructive/90 focus-visible:ring-destructive/20 dark:border-destructive-fill dark:bg-destructive-fill dark:hover:bg-destructive-fill/90 dark:focus-visible:ring-destructive/40",
        outline:
          "border border-border hover:border-foreground/40",
        secondary:
          "border border-border text-secondary-foreground hover:border-foreground/40",
        ghost:
          "hover:bg-accent hover:text-accent-foreground",
        link: "text-foreground underline underline-offset-4 hover:no-underline",
      },
      // The shared control height keeps actions aligned with fields and links.
      size: {
        default: "h-control px-gutter text-sm",
        xs: "h-7 px-2 text-xs [&_svg]:size-3",
        sm: "h-control px-2.5 text-sm",
        md: "h-10 px-gutter text-sm",
        lg: "h-11 px-page",
        icon: "size-control",
        "icon-xs": "size-7 [&_svg]:size-3",
        "icon-sm": "size-8",
        "icon-lg": "size-11",
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
