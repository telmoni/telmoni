"use client";

import { useRef, type KeyboardEvent } from "react";

import { cn } from "@/lib/utils";

export interface Choice<T extends string> {
  value: T;
  label: string;
  /** Shown, and never chosen: a choice this person may not make, or that is
   *  not open yet. The arrow keys pass over it, as they do a disabled radio. */
  disabled?: boolean;
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
  describedBy,
  options,
  value,
  onChange,
}: {
  /** The id of the element that names the group. */
  labelledBy: string;
  /** The id of what a screen reader should read after the name, if anything. */
  describedBy?: string;
  options: readonly Choice<T>[];
  value: T;
  onChange: (value: T) => void;
}) {
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);
  const at = options.findIndex((o) => o.value === value);
  // The one Tab stop: the checked segment, or the first open one when the
  // checked segment cannot take the focus.
  const stop = at >= 0 && !options[at]?.disabled ? at : options.findIndex((o) => !o.disabled);

  /** The nearest open choice `step` away from `from`, wrapping; -1 for none. */
  function nextOpen(from: number, step: 1 | -1): number {
    const n = options.length;
    for (let k = 1; k <= n; k++) {
      const i = (((from + step * k) % n) + n) % n;
      if (!options[i]?.disabled) return i;
    }
    return -1;
  }

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    // From the focused segment, which is the checked one unless the checked
    // one is closed and the focus sits on the Tab stop instead.
    const focused = buttons.current.findIndex((b) => b !== null && b === e.target);
    const from = focused >= 0 ? focused : at;
    const forward = e.key === "ArrowRight" || e.key === "ArrowDown";
    const back = e.key === "ArrowLeft" || e.key === "ArrowUp";
    const to =
      e.key === "Home"
        ? options.findIndex((o) => !o.disabled)
        : e.key === "End"
          ? options.findLastIndex((o) => !o.disabled)
          : forward
            ? nextOpen(from, 1)
            : back
              ? nextOpen(from, -1)
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
      aria-describedby={describedBy}
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
            disabled={option.disabled}
            tabIndex={i === stop ? 0 : -1}
            onClick={() => onChange(option.value)}
            className={cn(
              "h-7 cursor-pointer px-2.5 text-xs font-medium transition-colors disabled:cursor-not-allowed",
              // The checked segment is never dimmed: closed or not, it is the
              // answer the group gives.
              checked
                ? "bg-secondary text-foreground"
                : "text-muted-foreground enabled:hover:text-foreground disabled:opacity-50",
            )}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}
