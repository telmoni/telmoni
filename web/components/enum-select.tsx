"use client";

import type { ReactNode } from "react";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

/** A select over a closed set of words. What it hands back is one of
 *  `options`, never a string asserted into the type: a renamed variant or a
 *  stray item fails here, not at the action that refuses it. */
export function EnumSelect<T extends string>({
  id,
  value,
  options,
  label,
  onChange,
  disabled,
  className = "w-full max-w-md",
  ariaLabel,
  describedBy,
}: {
  id?: string;
  value: T;
  options: readonly T[];
  label: (option: T) => ReactNode;
  onChange: (next: T) => void;
  disabled?: boolean;
  className?: string;
  ariaLabel?: string;
  describedBy?: string;
}) {
  return (
    <Select
      value={value}
      disabled={disabled}
      onValueChange={(next) => {
        const chosen = options.find((option) => option === next);
        if (chosen !== undefined) onChange(chosen);
      }}
    >
      <SelectTrigger
        id={id}
        className={className}
        aria-label={ariaLabel}
        aria-describedby={describedBy}
      >
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {options.map((option) => (
          <SelectItem key={option} value={option}>
            {label(option)}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
