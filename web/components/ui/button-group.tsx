import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"

import { cn } from "@/lib/utils"

const buttonGroupVariants = cva(
  "flex items-center gap-gutter",
  {
    variants: {
      padded: {
        true: "px-gutter",
        false: "",
      },
      wrap: {
        true: "flex-wrap",
        false: "",
      },
    },
    defaultVariants: {
      padded: false,
      wrap: false,
    },
  }
)

export interface ButtonGroupProps
  extends React.ComponentProps<"div">,
    VariantProps<typeof buttonGroupVariants> {}

export function ButtonGroup({
  className,
  padded,
  wrap,
  ...props
}: ButtonGroupProps) {
  return (
    <div
      role="group"
      data-slot="button-group"
      className={cn(buttonGroupVariants({ padded, wrap }), className)}
      {...props}
    />
  )
}
