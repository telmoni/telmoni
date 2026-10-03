"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { LocalTime } from "@/components/local-time";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { organizationLabel, personName } from "@/lib/identity";
import type { OrganizationEntry } from "@/lib/server/entities/organization";

import { acceptOwnershipAction, declineOwnershipAction } from "./ownership-actions";

/// Organizations whose owner has offered them to you. Each one says plainly
/// what taking it over means before the button that does it.
export function OwnershipOffersSection({
  offers,
}: {
  offers: readonly OrganizationEntry[];
}) {
  if (offers.length === 0) return null;
  return (
    <section className="grid gap-3" data-testid="ownership-offers">
      <div className="flex items-center gap-2">
        <h2 className="text-sm font-medium">Ownership offers</h2>
        <Badge variant="secondary" className="text-xs">
          {offers.length}
        </Badge>
      </div>
      {offers.map((offer) => (
        <OwnershipOffer key={offer.organizationId} offer={offer} />
      ))}
    </section>
  );
}

function OwnershipOffer({ offer }: { offer: OrganizationEntry }) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  const label = organizationLabel(offer);
  const owner = offer.ownerEmail
    ? personName({ displayName: offer.ownerDisplayName, email: offer.ownerEmail })
    : "The owner";

  function answer(run: () => Promise<{ error: string | null }>) {
    setError(null);
    start(async () => {
      try {
        const r = await run();
        if (r.error) setError(r.error);
        else router.refresh();
      } catch {
        setError("Network error. Try again.");
      }
    });
  }

  return (
    <Card className="grid gap-3">
      <div className="grid gap-1">
        <p className="text-sm font-medium">
          {owner} wants to hand you {label}
        </p>
        <p className="text-sm text-muted-foreground">
          As its owner you can do what only an owner can: delete it, hand it on,
          and delete its projects or hand them to other organizations. {owner}{" "}
          stays on as an admin.
        </p>
        {offer.ownershipOfferExpiresAt && (
          <p className="text-xs text-muted-foreground">
            The offer lapses <LocalTime iso={offer.ownershipOfferExpiresAt} mode="date" />.
          </p>
        )}
      </div>
      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
      <div className="flex items-center gap-3">
        <ConfirmDialog
          trigger={
            <Button size="sm" disabled={pending}>
              Accept
            </Button>
          }
          title={`Become the owner of ${label}?`}
          description={`${owner} becomes an admin, and ${label} is yours.`}
          confirmLabel="Become the owner"
          onConfirm={() => answer(() => acceptOwnershipAction(offer.organizationId))}
        />
        <Button
          size="sm"
          variant="outline"
          disabled={pending}
          onClick={() => answer(() => declineOwnershipAction(offer.organizationId))}
        >
          Decline
        </Button>
      </div>
    </Card>
  );
}
