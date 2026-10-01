"use client";

import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";

import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

/**
 * A password field with a button that shows what was typed and hides it
 * again. Every password field on the sign-in, sign-up and reset pages is one,
 * so a person checking a long password reads it the same way on each.
 *
 * The button keeps one name and says its state through `aria-pressed`, so a
 * screen reader hears "Show password, pressed" rather than a label that
 * changes under it. It is `type="button"`, so pressing it never submits the
 * form, and `tabIndex` is left alone so a keyboard reaches it after the field.
 */
export function PasswordInput({
  className,
  id,
  ...props
}: Omit<React.ComponentProps<"input">, "type">) {
  const [visible, setVisible] = useState(false);
  return (
    <div className="relative">
      <Input
        id={id}
        type={visible ? "text" : "password"}
        className={cn("pr-10", className)}
        {...props}
      />
      <button
        type="button"
        aria-label="Show password"
        aria-pressed={visible}
        aria-controls={id}
        title={visible ? "Hide password" : "Show password"}
        onClick={() => setVisible((v) => !v)}
        className={cn(
          "absolute inset-y-0 right-0 flex w-10 items-center justify-center rounded-r-menu",
          "text-muted-foreground hover:text-foreground",
          "outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50",
        )}
      >
        {visible ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
      </button>
    </div>
  );
}
