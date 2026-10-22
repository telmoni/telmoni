"use client";

import Link from "next/link";
import { useRef } from "react";

import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { giverText, seatSubtitle } from "@/lib/notifications";
import type { GivenSeat } from "@/lib/types/given-seat";

import { useGivenSeat } from "./use-given-seat";

/// Projects the person was added to from an organization's roster, with
/// nothing to accept: each row leads to its project, and leaves once the
/// project is opened or the row dismissed.
export function GivenSeatsSection({ seats }: { seats: readonly GivenSeat[] }) {
  if (seats.length === 0) return null;
  return (
    <section className="grid gap-3" id="given-seats" data-testid="given-seats">
      <div className="flex items-center gap-2">
        <h2 className="text-sm font-medium">Projects you were added to</h2>
        <Badge variant="secondary" className="text-xs">
          {seats.length}
        </Badge>
      </div>
      <DataTable>
        <THead>
          <Th>Project</Th>
          <Th>Added by</Th>
          <Th>When</Th>
          <Th />
        </THead>
        <tbody>
          {seats.map((seat) => (
            <GivenSeatRow key={seat.projectId} seat={seat} />
          ))}
        </tbody>
      </DataTable>
    </section>
  );
}

function GivenSeatRow({ seat }: { seat: GivenSeat }) {
  const row = useRef<HTMLTableRowElement>(null);
  const { href, open, dismiss, error, pending, opening } = useGivenSeat(seat, {
    onGone: () => focusPastRow(row.current),
  });
  // Project names are an organization's own: two seats may share one.
  const named = `${seat.projectName} in ${seat.organizationName}`;

  return (
    <tr ref={row}>
      <Td className="font-medium">
        <span className="block">{seat.projectName}</span>
        <span className="block text-xs font-normal text-muted-foreground">
          {seatSubtitle(seat)}
        </span>
      </Td>
      <Td className="text-xs text-muted-foreground">{giverText(seat)}</Td>
      <Td className="text-xs text-muted-foreground whitespace-nowrap">
        <LocalTime iso={seat.createdAt} mode="date" />
      </Td>
      <Td className="text-right whitespace-nowrap">
        {error && (
          <span className="mr-2 text-xs text-destructive" role="alert">
            {error}
          </span>
        )}
        <div className="inline-flex items-center gap-3">
          <Button asChild size="sm" className="h-9 text-xs">
            <Link
              href={href}
              onClick={open}
              aria-label={`${opening ? "Opening" : "Open"} ${named}`}
            >
              {opening ? "Opening…" : "Open"}
            </Link>
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="h-9 text-xs"
            disabled={pending}
            onClick={dismiss}
            aria-label={`Dismiss ${named}`}
          >
            Dismiss
          </Button>
        </div>
      </Td>
    </tr>
  );
}

// The row goes with its focused button: the focus moves to the next row's
// Open, or the one above's, so Tab carries on from there, not the page's top.
function focusPastRow(row: HTMLTableRowElement | null) {
  if (!row?.contains(document.activeElement)) return;
  const near = row.nextElementSibling ?? row.previousElementSibling;
  near?.querySelector<HTMLElement>("a, button")?.focus();
}
