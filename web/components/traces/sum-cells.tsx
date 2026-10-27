import type { ReactNode } from "react";

import { LocalTime } from "@/components/local-time";
import type { UserRow } from "@/lib/server/entities/users";
import { formatCount } from "@/lib/traces";
import { cn } from "@/lib/utils";

// The cells a session's row and a user's share: what their runs added up to
// in the window, as the sessions and users tables show them.

/** A column of what a row's runs added up to. */
export type SumColumn =
  | "traces"
  | "errors"
  | "steps"
  | "input_tokens"
  | "output_tokens"
  | "tokens"
  | "first_seen"
  | "last_seen";

/** A row's sums, which a session's row and a user's both hold. */
type Sums = Omit<UserRow, "user_id">;

/** The cell's look: counts stand right, a failure in red, times muted. */
export function sumClass(column: SumColumn, row: Sums): string {
  switch (column) {
    case "first_seen":
    case "last_seen":
      return "whitespace-nowrap text-xs text-muted-foreground";
    case "errors":
      return cn(
        "whitespace-nowrap text-right tabular-nums",
        row.errors > 0 ? "text-destructive" : "text-muted-foreground",
      );
    default:
      return "whitespace-nowrap text-right tabular-nums";
  }
}

/** What the cell says on hover, where it says more than it shows. */
export function sumTitle(column: SumColumn, row: Sums): string | undefined {
  return column === "tokens"
    ? `${formatCount(row.input_tokens)} in, ${formatCount(row.output_tokens)} out`
    : undefined;
}

/** The cell itself. */
export function sumCell(column: SumColumn, row: Sums): ReactNode {
  switch (column) {
    case "tokens":
      return formatCount(row.input_tokens + row.output_tokens);
    case "first_seen":
    case "last_seen":
      return <LocalTime iso={row[column]} />;
    default:
      return formatCount(row[column]);
  }
}
