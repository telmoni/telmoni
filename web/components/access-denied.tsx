import Link from "next/link";

import { Button } from "@/components/ui/button";

/**
 * The page-level refusal: this account is signed in and the page is not theirs.
 *
 * Distinct from `RoleRestricted`, which is the SECTION-level version — one card
 * inside a page the caller can otherwise use. This one replaces the page,
 * because a member standing in somebody else's organization has no business on
 * it at all, and half a page with one restricted panel reads as a bug.
 *
 * Distinct from `ServiceUnavailable` in the thing it must not do: offer a
 * retry. Nothing about pressing again changes a role, and a "Try again" button
 * on a permission refusal teaches people to keep pressing it.
 */
export function AccessDenied({
  what,
  owner = null,
  backHref = "/organization",
  backLabel = "Back to overview",
}: {
  what: string;
  /// The organization owner's name, else their address, from `ownerContact`;
  /// `null` when `/me` did not carry them. The owner, and not "an admin": at
  /// the organization level only the owner changes a role.
  owner?: string | null;
  backHref?: string;
  backLabel?: string;
}) {
  return (
    <div
      className="flex max-w-md flex-col items-start gap-4 pb-8"
      role="alert"
      data-testid="access-denied"
    >
      {/* The height of `PageHeader`'s row, so the refusal starts where every
          other page's title does. */}
      <p className="flex min-h-8 items-center text-xs tracking-label text-brand-negative uppercase">
        access denied
      </p>
      <h1 className="text-xl font-light tracking-tight">
        Your role doesn&rsquo;t include {what}.
      </h1>
      <p className="text-sm leading-relaxed text-muted-foreground">
        {owner ? (
          <>
            Contact {owner}, the organization&rsquo;s owner, to request access.
          </>
        ) : (
          <>Contact the organization&rsquo;s owner to request access.</>
        )}
      </p>
      <Button asChild variant="outline">
        <Link href={backHref}>{backLabel}</Link>
      </Button>
    </div>
  );
}
