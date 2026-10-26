"use client";

import Link from "next/link";
import { unstable_rethrow } from "next/navigation";
import { Bell, Check, ChevronRight, Download, Loader2, X } from "lucide-react";
import { useId, useRef, useState, useTransition } from "react";

import { downloadExport, useAuditExportsWatcher } from "@/components/audit-exports";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { exportSummary } from "@/lib/audit-exports";
import { NOTIFICATIONS_HREF } from "@/lib/console-nav";
import { NOTIFICATIONS_KEY, sequenceHint } from "@/lib/keys";
import {
  badgeCount,
  bellLabel,
  giverText,
  inviteSubtitle,
  inviterText,
  seatSubtitle,
} from "@/lib/notifications";
import { organizationLabel } from "@/lib/identity";
import { administersOrganization } from "@/lib/organization-role";
import { usePageCovered } from "@/lib/page-cover";
import { organizationPath } from "@/lib/slug";
import type { OrganizationEntry, ProjectOffer } from "@/lib/server/entities/organization";
import {
  useActiveOrganization,
  useAuditExports,
  useForgetAuditExport,
  useGivenSeats,
  useIncomingInvites,
  useMarkAuditExportDownloaded,
  useOrganizations,
  useProjectOffers,
  useRemoveIncomingInvite,
} from "@/lib/store";
import { useLetterKeys } from "@/lib/use-accessibility";
import { isBuilding, isWaiting, type AuditExport } from "@/lib/types/audit-export";
import type { GivenSeat } from "@/lib/types/given-seat";
import type { IncomingInvite } from "@/lib/types/incoming-invite";
import { cn } from "@/lib/utils";

import {
  acceptIncomingInviteAction,
  declineIncomingInviteAction,
} from "@/app/(app)/account/notifications/invite-actions";
import { useGivenSeat } from "@/app/(app)/account/notifications/use-given-seat";

export function NotificationsBell() {
  useAuditExportsWatcher();
  const invites = useIncomingInvites() ?? [];
  const seats = useGivenSeats() ?? [];
  const offers = useOrganizations().filter((o) => o.ownershipOfferExpiresAt);
  const projectOffers = useProjectOffers() ?? [];
  // The person's audit log exports of the organization the console stands
  // in, while they still administer it: a file waiting to be taken counts,
  // one still building only shows.
  const organization = useActiveOrganization();
  const auditExports = useAuditExports();
  const exportsHere =
    organization && administersOrganization(organization.role)
      ? (auditExports[organization.organizationId] ?? [])
      : [];
  const exportsWaiting = exportsHere.filter(isWaiting);
  const exportsBuilding = exportsHere.filter(isBuilding);
  const count =
    invites.length + seats.length + offers.length + projectOffers.length + exportsWaiting.length;
  const badge = badgeCount(count);
  const letterKeys = useLetterKeys();
  const covered = usePageCovered();
  // Held here so a row that leads away can close the menu before it goes.
  const [open, setOpen] = useState(false);

  return (
    <DropdownMenu modal={false} open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger
        data-testid="notifications-bell"
        aria-label={bellLabel(count)}
        title={
          letterKeys && !covered
            ? `Notifications (${sequenceHint(NOTIFICATIONS_KEY)})`
            : "Notifications"
        }
        className={cn(
          "relative flex size-9 shrink-0 items-center justify-center",
          "rounded-md text-muted-foreground hover:text-foreground",
          "outline-none focus-visible:ring-2 focus-visible:ring-ring",
          "hover:bg-sidebar-accent/50",
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
        // Right under the header's rule, as the switchers' menus open
        // (`resource-selector.tsx`).
        sideOffset={12.5}
        data-testid="notifications-menu"
      >
        {/* The head is the account menu's: one 36px line in the label's small
            muted type, then the rule, so the header's menus open alike. */}
        <DropdownMenuLabel className="flex h-9 items-center justify-between gap-2 px-2 py-0 text-xs font-normal text-muted-foreground">
          <span>Notifications</span>
          {count > 0 && (
            <Badge variant="secondary" className="h-5 px-1.5 text-[10px]">
              {count}
            </Badge>
          )}
        </DropdownMenuLabel>
        <DropdownMenuSeparator />

        {count === 0 && exportsBuilding.length === 0 ? (
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
            {organization &&
              exportsWaiting.map((entry) => (
                <ExportReadyRow key={entry.id} entry={entry} organization={organization.slug} />
              ))}
            {organization &&
              exportsBuilding.map((entry) => (
                <ExportBuildingRow key={entry.id} entry={entry} organization={organization.slug} />
              ))}
            {offers.map((offer) => (
              <OfferRow key={offer.organizationId} offer={offer} />
            ))}
            {projectOffers.map((offer) => (
              <ProjectOfferRow key={offer.projectId} offer={offer} />
            ))}
            {seats.map((seat) => (
              <GivenSeatRow key={seat.projectId} seat={seat} onLeave={() => setOpen(false)} />
            ))}
            {invites.map((invite) => (
              <InviteRow key={invite.id} invite={invite} />
            ))}
          </div>
        )}

        {/* The foot is every header menu's foot (`nav-user.tsx`,
            `resource-selector.tsx`): a rule, then one 36px row, its icon
            first. The rows above drop their last rule for it. */}
        <DropdownMenuSeparator />
        <DropdownMenuItem asChild className="h-9 cursor-pointer gap-2 px-2">
          <Link href={NOTIFICATIONS_HREF} data-testid="notifications-menu-all">
            <Bell className="size-4 shrink-0 text-muted-foreground" />
            Go to notifications
          </Link>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// A finished export of the audit log, taken from here: selecting the row
// downloads the file, and once the browser has it the bell stops offering it,
// as it does at once for one no longer kept. A taken file stays for the rest
// of its week, in the export dialog's list.
function ExportReadyRow({ entry, organization }: { entry: AuditExport; organization: string }) {
  const markDownloaded = useMarkAuditExportDownloaded();
  const forget = useForgetAuditExport();
  const [now] = useState(() => new Date());
  return (
    <DropdownMenuItem
      className="h-auto cursor-pointer rounded-none border-b border-border px-3 py-3 last:border-b-0"
      data-testid="notifications-menu-export"
      onSelect={() => {
        void downloadExport(organization, entry.id).then((result) => {
          if (result === "taken") markDownloaded(entry.id);
          if (result === "gone") forget(entry.id);
        });
      }}
    >
      <span className="grid flex-1 gap-0.5">
        <span className="truncate text-sm font-medium">Audit log export ready</span>
        <span className="truncate text-xs text-muted-foreground">
          {exportSummary(entry, now)}
        </span>
      </span>
      <Download className="size-4 shrink-0 text-muted-foreground" />
    </DropdownMenuItem>
  );
}

// One still building: an item like the others, so the arrow keys reach it and
// a screen reader says it, and it opens the audit log page, where the export
// dialog lists it.
function ExportBuildingRow({ entry, organization }: { entry: AuditExport; organization: string }) {
  const [now] = useState(() => new Date());
  return (
    <DropdownMenuItem
      asChild
      className="h-auto rounded-none border-b border-border px-3 py-3 last:border-b-0"
    >
      <Link
        href={organizationPath(organization, "/audit-log")}
        data-testid="notifications-menu-export-building"
      >
        <span className="grid flex-1 gap-0.5">
          <span className="truncate text-sm font-medium">Preparing your audit log export</span>
          <span className="truncate text-xs text-muted-foreground">
            {exportSummary(entry, now)}
          </span>
        </span>
        <Loader2 aria-hidden className="size-4 shrink-0 animate-spin text-muted-foreground" />
      </Link>
    </DropdownMenuItem>
  );
}

// ⚠ **An offer is answered on the page, not from here.** Taking an
// organization over hands you its roster and its deletion, and the page says
// so before the button that does it; the bell only points there.
// `h-auto` undoes the menu item's fixed `h-9`: these rows hold two lines, and
// a row clipped to one line's height overflowed the list into a scrollbar.
// The last row drops its rule, which would double the one above the foot.
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

// A project the person was added to from a roster, with nothing to answer.
// Both buttons are menu items, so the keyboard reaches them: Open closes the
// menu and marks the seat seen before it leads to the project; Dismiss marks
// it and leaves the menu open, the focus moved on before the row goes.
function GivenSeatRow({ seat, onLeave }: { seat: GivenSeat; onLeave: () => void }) {
  const row = useRef<HTMLDivElement>(null);
  const about = useId();
  const { href, open, dismiss, error, pending } = useGivenSeat(seat, {
    onLeave,
    onGone: () => focusPast(row.current),
  });
  // Project names are an organization's own: two seats may share one.
  const named = `${seat.projectName} in ${seat.organizationName}`;

  return (
    <div
      ref={row}
      className="grid gap-2 border-b border-border px-3 py-3 last:border-b-0"
      data-testid="notifications-menu-seat"
    >
      {/* `min-w-0`, or the grid sizes the row to its longest line and a long
          name runs past the menu instead of truncating. */}
      <div id={about} className="grid min-w-0 gap-0.5">
        <span className="truncate text-sm font-medium">{seat.projectName}</span>
        <span className="truncate text-xs text-muted-foreground">
          You were added · {seatSubtitle(seat)}
        </span>
        <span className="truncate text-xs text-muted-foreground/70">by {giverText(seat)}</span>
      </div>

      {error && (
        <p className="text-xs text-destructive" role="alert">
          {error}
        </p>
      )}

      <div className="flex items-center gap-3">
        <DropdownMenuItem
          asChild
          className={cn(
            buttonVariants({ size: "sm" }),
            // The icon takes the button's colour, as the Accept beside it does.
            "h-7 flex-1 gap-1 text-xs focus:bg-primary/90 focus:text-primary-foreground [&_svg:not([class*='text-'])]:text-current",
          )}
        >
          <Link
            href={href}
            onClick={open}
            aria-label={`Open ${named}`}
            aria-describedby={about}
          >
            <ChevronRight className="size-3.5" />
            Open
          </Link>
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={pending}
          // The menu stays open for the rows still waiting.
          onSelect={(e) => {
            e.preventDefault();
            dismiss();
          }}
          aria-label={`Dismiss ${named}`}
          aria-describedby={about}
          className={cn(
            buttonVariants({ variant: "outline", size: "sm" }),
            "mt-0! h-7 flex-1 gap-1 text-xs text-muted-foreground hover:text-foreground focus:text-foreground [&_svg:not([class*='text-'])]:text-current",
          )}
        >
          <X className="size-3.5" />
          Dismiss
        </DropdownMenuItem>
      </div>
    </div>
  );
}

// The menu holds no focus of its own (`modal={false}`): a focused item gone
// with its row would leave the focus on the page, out of the arrow keys'
// reach. It moves to the next item first, or to the menu, which hands it on.
function focusPast(row: HTMLElement | null) {
  if (!row?.contains(document.activeElement)) return;
  const menu = row.closest<HTMLElement>("[role='menu']");
  const next = Array.from(menu?.querySelectorAll<HTMLElement>("[role='menuitem']") ?? []).find(
    (item) =>
      !row.contains(item) &&
      !item.hasAttribute("data-disabled") &&
      Boolean(row.compareDocumentPosition(item) & Node.DOCUMENT_POSITION_FOLLOWING),
  );
  (next ?? menu)?.focus();
}

// ⚠ **A row, not a `DropdownMenuItem`.** A menu item closes the menu when it is
// selected, and every click inside it counts — so Accept would settle one
// invitation and then take the other three off the screen with it. A plain row
// holding two real buttons keeps the menu open, which is the whole point of
// answering from here rather than from the page.
function InviteRow({ invite }: { invite: IncomingInvite }) {
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
        // The row leaves at once; the projects the accept just granted come
        // with the page the action's revalidation draws again.
        removeIncomingInvite(invite.id);
      } catch (err) {
        unstable_rethrow(err);
        setError(failure);
      }
    });
  }

  return (
    <div
      className="grid gap-2 border-b border-border px-3 py-3 last:border-b-0"
      data-testid="notifications-menu-invite"
    >
      <div className="grid min-w-0 gap-0.5">
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
