import type { ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { WINDOWS } from "@/lib/list-query";

// The window a list reads, chosen with a plain GET form: the browser writes
// the URL, the server renders the list, and nothing runs on the client. The
// sort in force rides along unchanged, in hidden fields.

const SELECT =
  "h-8 rounded-md border border-input bg-transparent px-2 text-xs shadow-xs outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50";

/** A native select, styled as the console's inputs are: what a form that
 *  must work without a script offers where there is a list to choose from. */
export function SelectField({
  label,
  name,
  defaultValue,
  children,
}: {
  label: string;
  name: string;
  defaultValue: string;
  children: ReactNode;
}) {
  return (
    <label className="grid gap-1 text-xs text-muted-foreground">
      {label}
      <select name={name} defaultValue={defaultValue} className={SELECT}>
        {children}
      </select>
    </label>
  );
}

/** The windows a page offers, and the one in force among them: a window the
 *  URL asked for by hand is offered too, so the select says what is shown. */
export function WindowSelect({ days }: { days: number }) {
  const windows: number[] = WINDOWS.some((w) => w === days)
    ? [...WINDOWS]
    : [...WINDOWS, days].sort((a, b) => a - b);
  return (
    <SelectField label="Window" name="days" defaultValue={String(days)}>
      {windows.map((w) => (
        <option key={w} value={w}>
          {w === 1 ? "Last day" : `Last ${w} days`}
        </option>
      ))}
    </SelectField>
  );
}

/** The window alone, for a list with nothing else to filter by. `carry` is
 *  what else the URL holds that the new one keeps: the sort in force. */
export function WindowForm({
  action,
  days,
  carry = {},
  label,
}: {
  action: string;
  days: number;
  carry?: Record<string, string | undefined>;
  label: string;
}) {
  return (
    <form method="get" action={action} className="flex flex-wrap items-end gap-2" aria-label={label}>
      <WindowSelect days={days} />
      {Object.entries(carry).map(([name, value]) =>
        value ? <input key={name} type="hidden" name={name} value={value} /> : null,
      )}
      <Button type="submit" size="sm" variant="outline">
        Show
      </Button>
    </form>
  );
}
