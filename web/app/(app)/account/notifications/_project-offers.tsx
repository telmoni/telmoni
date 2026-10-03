"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";

import { LocalTime } from "@/components/local-time";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { organizationLabel, personName } from "@/lib/identity";
import type { OrganizationEntry, ProjectOffer } from "@/lib/server/entities/organization";

import { acceptProjectOfferAction, declineProjectOfferAction } from "./ownership-actions";

/// Projects whose owner has offered them to you. Each one says plainly what
/// taking it means — what comes with it, what stays behind — before the
/// button that does it, and asks which of your organizations it lands in
/// when you own several.
export function ProjectOffersSection({
  offers,
  owned,
}: {
  offers: readonly ProjectOffer[];
  /// The organizations the person owns: where an accepted project may land.
  owned: readonly OrganizationEntry[];
}) {
  if (offers.length === 0) return null;
  return (
    <section className="grid gap-3" data-testid="project-offers">
      <div className="flex items-center gap-2">
        <h2 className="text-sm font-medium">Project offers</h2>
        <Badge variant="secondary" className="text-xs">
          {offers.length}
        </Badge>
      </div>
      {offers.map((offer) => (
        <ProjectOfferCard key={offer.projectId} offer={offer} owned={owned} />
      ))}
    </section>
  );
}

function ProjectOfferCard({
  offer,
  owned,
}: {
  offer: ProjectOffer;
  owned: readonly OrganizationEntry[];
}) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [pending, start] = useTransition();

  const from = organizationLabel({ name: offer.organizationName });
  const owner = offer.ownerEmail
    ? personName({ displayName: offer.ownerDisplayName, email: offer.ownerEmail })
    : "The owner";

  function decline() {
    setError(null);
    start(async () => {
      try {
        const r = await declineProjectOfferAction(offer.projectId);
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
          {owner} wants to hand you the project {offer.name}
        </p>
        <p className="text-sm text-muted-foreground">
          It leaves {from} and moves into an organization you own, with its
          members and its API keys — rotate any key you did not mint. Its
          Slack, Discord and webhook connectors stay behind. {owner} stays on
          as an admin of the project, and joins your organization as a member.
          If a project there already has its name, it arrives with the next
          free number on the end.
        </p>
        <p className="text-xs text-muted-foreground">
          The offer lapses <LocalTime iso={offer.expiresAt} mode="date" />.
        </p>
      </div>
      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
      <div className="flex items-center gap-3">
        {owned.length === 0 ? (
          <p className="text-sm text-muted-foreground" data-testid="project-offer-nowhere">
            You own no organization to take it into.
          </p>
        ) : (
          <>
            <Button size="sm" disabled={pending} onClick={() => setOpen(true)}>
              Accept
            </Button>
            <AcceptDialog
              key={`accept-${offer.projectId}-${open}`}
              offer={offer}
              owned={owned}
              open={open}
              onOpenChange={setOpen}
              onError={setError}
            />
          </>
        )}
        <Button size="sm" variant="outline" disabled={pending} onClick={decline}>
          Decline
        </Button>
      </div>
    </Card>
  );
}

// Taking a project hands you its keys and its members and makes its previous
// owner a member of yours, so it is confirmed first — and when you own more
// than one organization, this is where you say which one it lands in.
function AcceptDialog({
  offer,
  owned,
  open,
  onOpenChange,
  onError,
}: {
  offer: ProjectOffer;
  owned: readonly OrganizationEntry[];
  open: boolean;
  onOpenChange: (v: boolean) => void;
  onError: (message: string | null) => void;
}) {
  const router = useRouter();
  const [destination, setDestination] = useState(owned[0]?.organizationId ?? "");
  const [pending, start] = useTransition();
  const chosen = owned.find((o) => o.organizationId === destination) ?? owned[0];

  function confirm() {
    onError(null);
    start(async () => {
      try {
        const r = await acceptProjectOfferAction(offer.projectId, destination);
        if (r.error) {
          onError(r.error);
          onOpenChange(false);
        } else if (r.href) {
          router.push(r.href);
        } else {
          router.refresh();
        }
      } catch {
        onError("Network error. Try again.");
        onOpenChange(false);
      }
    });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Take over {offer.name}?</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <DialogDescription>
            {owned.length > 1
              ? `It moves into the organization you pick, with its members and API keys, and ${chosen ? organizationLabel(chosen) : "that organization"} becomes its home.`
              : `It moves into ${chosen ? organizationLabel(chosen) : "your organization"}, with its members and API keys.`}{" "}
            Its connectors stay behind, and its previous owner stays on as an
            admin of the project and a member of that organization.
          </DialogDescription>
          {owned.length > 1 && (
            <div className="grid gap-1.5">
              <Label htmlFor={`project-offer-destination-${offer.projectId}`}>
                Which organization
              </Label>
              <Select value={destination} onValueChange={setDestination}>
                <SelectTrigger
                  id={`project-offer-destination-${offer.projectId}`}
                  className="w-full"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {owned.map((o) => (
                    <SelectItem key={o.organizationId} value={o.organizationId}>
                      {organizationLabel(o)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}
        </DialogBody>
        <DialogFooter>
          <Button onClick={confirm} disabled={pending || !destination}>
            {pending ? "Taking it over…" : "Take it over"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
