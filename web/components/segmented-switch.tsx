"use client";

import type { LucideIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

// The track carries the shape, never the segments: each segment is
// `rounded-none` and the track's `overflow-hidden` rounds the two ends.
// `nav-user.tsx` draws the same track around its menu-item segments.

export interface Segment<T extends string> {
  value: T;
  label: string;
  Icon: LucideIcon;
}

/**
 * One choice spread across touching segments: theme, corners, anything with a
 * handful of options and an icon apiece.
 *
 * `value` is deliberately `string | null` rather than `T`. A caller whose
 * source has not answered yet — `next-themes` before it has read storage —
 * passes `null` so that no segment reads as pressed, instead of guessing a
 * default the next paint would contradict.
 */
export function SegmentedSwitch<T extends string>({
  label,
  options,
  value,
  onChange,
}: {
  label: string;
  options: readonly Segment<T>[];
  value: string | null;
  onChange: (value: T) => void;
}) {
  return (
    <div
      className="flex items-center overflow-hidden rounded-md border border-border"
      role="group"
      aria-label={label}
      // Its frame clips the segments, so Show focus draws their ring inside.
      data-segmented=""
    >
      {options.map(({ value: option, label: optionLabel, Icon }) => {
        const active = value === option;
        return (
          <Button
            key={option}
            type="button"
            variant="ghost"
            size="icon"
            aria-pressed={active}
            aria-label={optionLabel}
            title={optionLabel}
            onClick={() => onChange(option)}
            className={cn(
              "size-7 rounded-none",
              active
                ? "bg-secondary text-foreground"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            <Icon className="size-3.5" aria-hidden />
          </Button>
        );
      })}
    </div>
  );
}
