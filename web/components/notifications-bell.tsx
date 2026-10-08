"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { Bell, Check, ChevronRight, X } from "lucide-react";
import { useState, useTransition } from "react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { NOTIFICATIONS_HREF } from "@/lib/console-nav";
import {
  badgeCount,
  bellLabel,
  inviteSubtitle,
  inviterText,
} from "@/lib/notifications";
import { organizationLabel } from "@/lib/identity";
import type { OrganizationEntry, ProjectOffer } from "@/lib/server/entities/organization";
import {
  useIncomingInvites,
  useOrganizations,
  useProjectOffers,
  useRemoveIncomingInvite,
} from "@/lib/store";
import type { IncomingInvite } from "@/lib/types/incoming-invite";
import { cn } from "@/lib/utils";

import {
  acceptIncomingInviteAction,
  declineIncomingInviteAction,
} from "@/app/(app)/account/notifications/invite-actions";

export function NotificationsBell() {
  const invites = useIncomingInvites() ?? [];
  const offers = useOrganizations().filter((o) => o.ownershipOfferExpiresAt);
  const projectOffers = useProjectOffers() ?? [];
  const count = invites.length + offers.length + projectOffers.length;
  const badge = badgeCount(count);

  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger
        data-testid="notifications-bell"
        aria-label={bellLabel(count)}
        className={cn(
          "relative flex size-8 shrink-0 items-center justify-center",
          "rounded-md text-muted-foreground hover:text-foreground",
          "outline-none focus-visible:ring-2 focus-visible:ring-ring",
          "hover:bg-sidebar-accent",
        )}
      >
        <Bell className="size-4" />
        {badge && (
          <span
            aria-hidden
            data-testid="notifications-bell-count"
            className="absolute top-1 right-1 flex size-3.5 items-center justify-center rounded-full bg-primary text-[9px] font-medium text-primary-foreground ring-2 ring-background"
          >
            {badge}
          </span>
        )}
      </DropdownMenuTrigger>

      {/* ⚠ **The panel keeps its `p-1`.** That inset is what sets a row's
          `rounded-md` corner 4px inside the panel's `rounded-menu` one — the
          concentric curve — so overriding it to `p-0` would square off every
          hover in this menu. Anything that needs the full width bleeds back
          out with `-mx-1` instead — the same trick `DropdownMenuSeparator`
          uses. */}
      <DropdownMenuContent
        className="w-80 animate-none!"
        align="end"
        sideOffset={14}
        data-testid="notifications-menu"
      >
        {/* `h-12` + the panel's `p-1` + the separator's `mt-1` = 56 to the
            rule, the height every overlay's first row lands on. */}
        <DropdownMenuLabel className="flex h-12 items-center justify-between gap-2">
          <span>Notifications</span>
          {count > 0 && (
            <Badge variant="secondary" className="h-5 px-1.5 text-[10px]">
              {count}
            </Badge>
          )}
        </DropdownMenuLabel>
        <DropdownMenuSeparator />

        {count === 0 ? (
          <p
            className="px-2 py-6 text-center text-sm text-muted-foreground"
            data-testid="notifications-menu-empty"
          >
            You&rsquo;re all caught up.
          </p>
        ) : (
          // The bleed sits on the scroller, not on each row: a row carrying its
          // own negative margin would push past a scroll container and earn a
          // horizontal scrollbar, since a box scrolling on one axis cannot keep
          // the other visible.
          <div className="-mx-1 max-h-96 overflow-y-auto">
            {offers.map((offer) => (
              <OfferRow key={offer.organizationId} offer={offer} />
            ))}
            {projectOffers.map((offer) => (
              <ProjectOfferRow key={offer.projectId} offer={offer} />
            ))}
            {invites.map((invite) => (
              <InviteRow key={invite.id} invite={invite} />
            ))}
          </div>
        )}

        {/* ⚠ **The footer is the account menu's footer, not one more row.**
            Same classes as the account menu's sign-out row, and the
            negative margins cancel the panel's `p-1` so the fill reaches three
            edges: a row highlighted as an inset pill here would be the one
            menu in the console whose last entry is shaped differently from
            every other menu's. Its `border-t` IS the rule, which is why there
            is no separator above it. */}
        <DropdownMenuItem
          asChild
          className="-mx-1 mt-1 -mb-1 h-14 cursor-pointer gap-3 rounded-none border-t px-3"
        >
          <Link href={NOTIFICATIONS_HREF} data-testid="notifications-menu-all">
            <span className="flex-1">Go to notifications</span>
            <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
          </Link>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// ⚠ **An offer is answered on the page, not from here.** Taking an
// organization over hands you its roster and its deletion, and the page says
// so before the button that does it; the bell only points there.
// `h-auto` undoes the menu item's fixed `h-8`: these rows hold two lines, and
// a row clipped to one line's height overflowed the list into a scrollbar.
// The last row drops its rule, which would double the footer's `border-t`.
function OfferRow({ offer }: { offer: OrganizationEntry }) {
  return (
    <DropdownMenuItem asChild className="h-auto rounded-none border-b border-border px-3 py-3 last:border-b-0">
      <Link href={NOTIFICATIONS_HREF} data-testid="notifications-menu-offer">
        <span className="grid flex-1 gap-0.5">
          <span className="truncate text-sm font-medium">{organizationLabel(offer)}</span>
          <span className="truncate text-xs text-muted-foreground">
            Ownership offered to you · review
          </span>
        </span>
        <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
      </Link>
    </DropdownMenuItem>
  );
}

// The same for a project: taking one over moves it into an organization of
// yours, with its keys, and the page says what comes with it first.
function ProjectOfferRow({ offer }: { offer: ProjectOffer }) {
  return (
    <DropdownMenuItem asChild className="h-auto rounded-none border-b border-border px-3 py-3 last:border-b-0">
      <Link href={NOTIFICATIONS_HREF} data-testid="notifications-menu-project-offer">
        <span className="grid flex-1 gap-0.5">
          <span className="truncate text-sm font-medium">{offer.name}</span>
          <span className="truncate text-xs text-muted-foreground">
            Project offered to you · review
          </span>
        </span>
        <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
      </Link>
    </DropdownMenuItem>
  );
}

// ⚠ **A row, not a `DropdownMenuItem`.** A menu item closes the menu when it is
// selected, and every click inside it counts — so Accept would settle one
// invitation and then take the other three off the screen with it. A plain row
// holding two real buttons keeps the menu open, which is the whole point of
// answering from here rather than from the page.
function InviteRow({ invite }: { invite: IncomingInvite }) {
  const router = useRouter();
  const removeIncomingInvite = useRemoveIncomingInvite();
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  function settle(
    run: () => Promise<{ error: string | null }>,
    failure: string,
  ) {
    setError(null);
    start(async () => {
      try {
        const res = await run();
        if (res.error) {
          setError(res.error);
          return;
        }
        // Drop it from the store first: the row leaves immediately, and the
        // refresh behind it is what re-reads the projects the accept just granted.
        removeIncomingInvite(invite.id);
        router.refresh();
      } catch {
        setError(failure);
      }
    });
  }

  return (
    <div
      className="grid gap-2 border-b border-border px-3 py-3 last:border-b-0"
      data-testid="notifications-menu-invite"
    >
      <div className="grid gap-0.5">
        <span className="truncate text-sm font-medium">
          {invite.targetName}
        </span>
        <span className="truncate text-xs text-muted-foreground">
          {inviteSubtitle(invite)}
        </span>
        <span className="truncate text-xs text-muted-foreground/70">
          from {inviterText(invite)}
        </span>
      </div>

      {error && (
        <p className="text-xs text-destructive" role="alert">
          {error}
        </p>
      )}

      <div className="flex items-center gap-3">
        <Button
          size="sm"
          className="h-7 flex-1 gap-1 text-xs"
          disabled={pending}
          onClick={() =>
            settle(
              () => acceptIncomingInviteAction(invite.id),
              "Couldn't accept that invitation. Try again.",
            )
          }
        >
          <Check className="size-3.5" />
          Accept
        </Button>
        {/* No confirmation step here, unlike the page's table. A dropdown that
            opens a dialog over itself loses the menu underneath, and declining
            is recoverable — an owner or admin can invite again. */}
        <Button
          variant="outline"
          size="sm"
          className="h-7 flex-1 gap-1 text-xs text-muted-foreground hover:text-foreground"
          disabled={pending}
          onClick={() =>
            settle(
              () => declineIncomingInviteAction(invite.id),
              "Couldn't decline that invitation. Try again.",
            )
          }
        >
          <X className="size-3.5" />
          Decline
        </Button>
      </div>
    </div>
  );
}
