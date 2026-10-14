import type { RosterCeiling } from "@/lib/server/entities/organization-member";

/**
 * Whether the organization holds as many people as its plan allows, those
 * invited among them: an invitation sent now would be refused, so a roster
 * page hides the button and says why instead.
 */
export function atCeiling(ceiling: RosterCeiling | null | undefined): ceiling is RosterCeiling {
  return ceiling != null && ceiling.held >= ceiling.limit;
}

/** What a roster page says in place of its invite button at the ceiling: the
 *  deployment's own sentence, which names what lifts it. */
export function MemberCeilingNotice({ ceiling }: { ceiling: RosterCeiling | null | undefined }) {
  if (!atCeiling(ceiling)) return null;
  return (
    <p
      data-testid="member-ceiling"
      className="rounded-md border border-border bg-muted/40 px-3 py-2 text-sm text-muted-foreground"
    >
      {ceiling.detail}
    </p>
  );
}
