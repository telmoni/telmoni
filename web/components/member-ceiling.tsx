import type { RosterCeiling } from "@/lib/server/entities/organization-member";

/** What a roster page says in place of its invite button at the ceiling: the
 *  deployment's own sentence, which names what lifts it. */
export function MemberCeilingNotice({ ceiling }: { ceiling: RosterCeiling | null | undefined }) {
  if (!ceiling?.reached) return null;
  return (
    <p
      data-testid="member-ceiling"
      className="rounded-md border border-border bg-muted/40 px-3 py-2 text-sm text-muted-foreground"
    >
      {ceiling.detail}
    </p>
  );
}
