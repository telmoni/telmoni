"use client";

import { useRef, type KeyboardEvent } from "react";

import { cn } from "@/lib/utils";

export interface Choice<T extends string> {
  value: T;
  label: string;
}

/**
 * One choice among a few, drawn as touching segments as `SegmentedSwitch` is,
 * but built as a radio group, WAI-ARIA's radio pattern: Tab reaches the chosen
 * segment alone, the arrow keys and Home and End move the choice, and a screen
 * reader hears a group of radios and which one is checked — what pressed
 * buttons cannot say of a choice that is always exactly one.
 */
export function ChoiceGroup<T extends string>({
  labelledBy,
  options,
  value,
  onChange,
}: {
  /** The id of the element that names the group. */
  labelledBy: string;
  options: readonly Choice<T>[];
  value: T;
  onChange: (value: T) => void;
}) {
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);
  const at = options.findIndex((o) => o.value === value);

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    const last = options.length - 1;
    const forward = e.key === "ArrowRight" || e.key === "ArrowDown";
    const back = e.key === "ArrowLeft" || e.key === "ArrowUp";
    const to =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? last
          : forward
            ? (at + 1) % options.length
            : back
              ? (at - 1 + options.length) % options.length
              : -1;
    if (to < 0) return;
    e.preventDefault();
    const next = options[to];
    if (!next) return;
    onChange(next.value);
    buttons.current[to]?.focus();
  }

  return (
    <div
      role="radiogroup"
      aria-labelledby={labelledBy}
      onKeyDown={onKeyDown}
      // Its frame clips the segments, so Show focus draws their ring inside.
      data-segmented=""
      className="flex shrink-0 items-center overflow-hidden rounded-md border border-border"
    >
      {options.map((option, i) => {
        const checked = option.value === value;
        return (
          <button
            key={option.value}
            ref={(el) => {
              buttons.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={checked}
            tabIndex={checked || (at === -1 && i === 0) ? 0 : -1}
            onClick={() => onChange(option.value)}
            className={cn(
              "h-7 cursor-pointer px-2.5 text-xs font-medium transition-colors",
              checked
                ? "bg-secondary text-foreground"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}
