"use client";

import Link from "next/link";
import { Bell, Check, ChevronRight, Download, Loader2, X } from "lucide-react";
import { useId, useRef, useState } from "react";

import { useAuditExportsWatcher, useTakeExport } from "@/components/audit-exports";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { LocalTime } from "@/components/local-time";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import type { ActionResult } from "@/lib/action-result";
import { exportSummary } from "@/lib/audit-exports";
import { NOTIFICATIONS_HREF } from "@/lib/console-nav";
import { NOTIFICATIONS_KEY, sequenceHint } from "@/lib/keys";
import {
  badgeCount,
  bellItems,
  bellLabel,
  giverText,
  inviteSubtitle,
  inviterText,
  noticeHref,
  seatSubtitle,
  type NoticePlace,
} from "@/lib/notifications";
import { organizationLabel } from "@/lib/identity";
import { usePageCovered } from "@/lib/page-cover";
import { organizationPath, projectPath } from "@/lib/slug";
import type { FeedItem } from "@/lib/server/data";
import type { OrganizationEntry, ProjectOffer } from "@/lib/server/entities/organization";
import {
  useActiveOrganization,
  useAuditExports,
  useGivenSeats,
  useIncomingInvites,
  useOrganizations,
  useProjectOffers,
  useRemoveIncomingInvite,
} from "@/lib/store";
import { useAction } from "@/lib/use-action";
import { useLetterKeys } from "@/lib/use-accessibility";
import { useBellPlaces, useScopeFeed, type BellScope } from "@/lib/use-scope-feed";
import type { AuditExport } from "@/lib/types/audit-export";
import type { GivenSeat } from "@/lib/types/given-seat";
import type { IncomingInvite } from "@/lib/types/incoming-invite";
import { cn, HEADER_MENU_OFFSET } from "@/lib/utils";

import {
  acceptIncomingInviteAction,
  declineIncomingInviteAction,
} from "@/app/(app)/settings/notifications/invite-actions";
import { useGivenSeat } from "@/app/(app)/settings/notifications/use-given-seat";

export function NotificationsBell() {
  useAuditExportsWatcher();
  const invites = useIncomingInvites() ?? [];
  const seats = useGivenSeats() ?? [];
  const projectOffers = useProjectOffers() ?? [];
  const organizations = useOrganizations();
  const organization = useActiveOrganization();
  const auditExports = useAuditExports();
  const { offers, exportsWaiting, exportsBuilding, count } = bellItems({
    invites,
    seats,
    projectOffers,
    organizations,
    organization,
    auditExports,
  });
  const letterKeys = useLetterKeys();
  const covered = usePageCovered();
  // Held here so a row that leads away can close the menu before it goes.
  const [open, setOpen] = useState(false);
  // One list: what is the person's own, then a preview of the notices of the
  // place they stand in — the project's, and the organization's for its
  // owners and admins — newest first, each leading to the page it is about.
  // The badge counts the person's own alone: a notice's read mark is shared
  // by everybody it reaches, so it is read, and marked, on its feed's page.
  const places = useBellPlaces();
  const projectFeed = useScopeFeed(places.project, open);
  const organizationFeed = useScopeFeed(places.organization, open);
  const feeds = [
    { ...projectFeed, scope: places.project },
    { ...organizationFeed, scope: places.organization },
  ].flatMap((read) => (read.scope ? [{ ...read, scope: read.scope }] : []));
  const notices = feeds
    .flatMap(({ scope, feed }) => (feed?.items ?? []).map((item) => ({ item, scope })))
    .toSorted((a, b) => (a.item.created_at < b.item.created_at ? 1 : a.item.created_at > b.item.created_at ? -1 : 0))
    .slice(0, NOTICE_ROWS);
  const reading = feeds.some((read) => !read.read);
  const badge = badgeCount(count);
  // Where "See all" goes: the Notifications page of where the console stands,
  // the project's before the organization's, else the person's own.
  const seeAll = places.project
    ? projectPath(places.project.organization, places.project.project, "/notifications")
    : places.organization
      ? organizationPath(places.organization.organization, "/notifications")
      : NOTIFICATIONS_HREF;
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
          "relative flex size-control shrink-0 items-center justify-center",
          "rounded-md text-muted-foreground hover:text-foreground",
          "hover:bg-sidebar-accent/50",
          "data-[state=open]:bg-sidebar-accent data-[state=open]:text-foreground data-[state=open]:hover:bg-sidebar-accent",
        )}
      >
        <Bell className="size-icon" />
        {badge && (
          <span
            aria-hidden
            data-testid="notifications-bell-count"
            className="absolute -top-0.5 -right-0.5 flex size-3.5 items-center justify-center rounded-full bg-primary text-xs leading-none font-medium text-primary-foreground ring-2 ring-background"
          >
            {badge}
          </span>
        )}
      </DropdownMenuTrigger>

      {/* Keep the panel inset around rounded rows; the scroller bleeds to its edges. */}
      <DropdownMenuContent
        className="w-80 animate-none!"
        align="end"
        sideOffset={HEADER_MENU_OFFSET}
        data-testid="notifications-menu"
      >
        {/* The head is the account menu's: one 28px line in the label's small
            muted type, then the rule, so the header's menus open alike. */}
        <DropdownMenuLabel className="flex h-control items-center justify-between gap-2 py-0">
          <span>Notifications</span>
          {count > 0 && (
            <Badge variant="secondary">
              {count}
            </Badge>
          )}
        </DropdownMenuLabel>
        <DropdownMenuSeparator />

        {count === 0 && exportsBuilding.length === 0 && notices.length === 0 ? (
          <p
            className="px-gutter py-page text-center text-sm text-muted-foreground"
            data-testid="notifications-menu-empty"
          >
            {reading ? "Reading…" : "You\u2019re all caught up."}
          </p>
        ) : (
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
            {notices.map(({ item, scope }) => (
              <NoticeRow key={`${scope.kind}:${item.id}`} item={item} scope={scope} onLeave={() => setOpen(false)} />
            ))}
          </div>
        )}

        {/* Match the other header menus' ruled action row. */}
        <DropdownMenuSeparator />
        <DropdownMenuItem asChild className="cursor-pointer">
          <Link href={seeAll} onClick={() => setOpen(false)} data-testid="notifications-menu-all">
            <Bell className="size-icon shrink-0 text-muted-foreground" />
            See all
          </Link>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** The most notices the menu lists; the feeds' own pages hold the rest. */
const NOTICE_ROWS = 20;

// One notice: where it was raised, its title, its body's first lines, and
// when, leading to the page it is about (`noticeHref`).
function NoticeRow({ item, scope, onLeave }: { item: FeedItem; scope: BellScope; onLeave: () => void }) {
  const unread = item.read_at === null;
  const place: NoticePlace =
    scope.kind === "project" ? { organization: scope.organization, project: scope.project } : { organization: scope.organization };
  return (
    <DropdownMenuItem
      asChild
      className="h-auto cursor-pointer items-start rounded-none border-b border-border px-gutter py-gutter last:border-b-0"
      data-testid="notifications-menu-notice"
    >
      <Link href={noticeHref(item, place)} onClick={onLeave}>
        <span className="grid min-w-0 flex-1 gap-0.5">
          <span className="truncate text-xs text-muted-foreground">
            {scope.kind === "project" ? scope.name : "Organization"}
          </span>
          <span className={cn("truncate text-sm", unread ? "font-medium" : "text-muted-foreground")}>
            {item.title}
            {unread && <span className="sr-only"> Unread</span>}
          </span>
          <span className="line-clamp-2 text-xs text-muted-foreground">{item.body}</span>
          <span className="text-xs text-muted-foreground">
            <LocalTime iso={item.created_at} mode="datetime" />
          </span>
        </span>
        <ChevronRight className="mt-0.5 size-icon shrink-0 text-muted-foreground" />
      </Link>
    </DropdownMenuItem>
  );
}

// A finished export of the audit log, taken from here: selecting the row
// downloads the file, and once the browser has it the bell stops offering it,
// as it does at once for one no longer kept. A taken file stays for the rest
// of its week, in the export dialog's list.
function ExportReadyRow({ entry, organization }: { entry: AuditExport; organization: string }) {
  const take = useTakeExport();
  const [now] = useState(() => new Date());
  return (
    <DropdownMenuItem
      className="h-auto cursor-pointer rounded-none border-b border-border px-gutter py-gutter last:border-b-0"
      data-testid="notifications-menu-export"
      onSelect={() => take(organization, entry.id)}
    >
      <span className="grid flex-1 gap-0.5">
        <span className="truncate text-sm font-medium">Audit log export ready</span>
        <span className="truncate text-xs text-muted-foreground">
          {exportSummary(entry, now)}
        </span>
      </span>
      <Download className="size-icon shrink-0 text-muted-foreground" />
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
      className="h-auto rounded-none border-b border-border px-gutter py-gutter last:border-b-0"
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
        <Loader2 aria-hidden className="size-icon shrink-0 animate-spin text-muted-foreground" />
      </Link>
    </DropdownMenuItem>
  );
}

// ⚠ **An offer is answered on the page, not from here.** Taking an
// organization over hands you its roster and its deletion, and the page says
// so before the button that does it; the bell only points there.
// `h-auto` undoes the menu item's fixed height: these rows hold two lines, and
// a row clipped to one line's height overflowed the list into a scrollbar.
// The last row drops its rule, which would double the one above the foot.
function OfferRow({ offer }: { offer: OrganizationEntry }) {
  return (
    <DropdownMenuItem asChild className="h-auto rounded-none border-b border-border px-gutter py-gutter last:border-b-0">
      <Link href={NOTIFICATIONS_HREF} data-testid="notifications-menu-offer">
        <span className="grid flex-1 gap-0.5">
          <span className="truncate text-sm font-medium">{organizationLabel(offer)}</span>
          <span className="truncate text-xs text-muted-foreground">
            Ownership offered to you · review
          </span>
        </span>
        <ChevronRight className="size-icon shrink-0 text-muted-foreground" />
      </Link>
    </DropdownMenuItem>
  );
}

// The same for a project: taking one over moves it into an organization of
// yours, with its keys, and the page says what comes with it first.
function ProjectOfferRow({ offer }: { offer: ProjectOffer }) {
  return (
    <DropdownMenuItem asChild className="h-auto rounded-none border-b border-border px-gutter py-gutter last:border-b-0">
      <Link href={NOTIFICATIONS_HREF} data-testid="notifications-menu-project-offer">
        <span className="grid flex-1 gap-0.5">
          <span className="truncate text-sm font-medium">{offer.name}</span>
          <span className="truncate text-xs text-muted-foreground">
            Project offered to you · review
          </span>
        </span>
        <ChevronRight className="size-icon shrink-0 text-muted-foreground" />
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
      className="grid gap-2 border-b border-border px-gutter py-gutter last:border-b-0"
      data-testid="notifications-menu-seat"
    >
      {/* `min-w-0`, or the grid sizes the row to its longest line and a long
          name runs past the menu instead of truncating. */}
      <div id={about} className="grid min-w-0 gap-0.5">
        <span className="truncate text-sm font-medium">{seat.projectName}</span>
        <span className="truncate text-xs text-muted-foreground">
          You were added · {seatSubtitle(seat)}
        </span>
        <span className="truncate text-xs text-muted-foreground">by {giverText(seat)}</span>
      </div>

      {error && (
        <p className="text-xs text-destructive" role="alert">
          {error}
        </p>
      )}

      <div className="flex items-center gap-gutter">
        <DropdownMenuItem
          asChild
          className={cn(
            buttonVariants({ size: "sm" }),
            // The icon takes the button's colour, as the Accept beside it does.
            "flex-1 gap-1 focus:bg-transparent focus:text-foreground [&_svg:not([class*='text-'])]:text-current",
          )}
        >
          <Link
            href={href}
            onClick={open}
            aria-label={`Open ${named}`}
            aria-describedby={about}
          >
            <ChevronRight />
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
            "mt-0! flex-1 gap-1 text-muted-foreground hover:text-foreground focus:text-foreground [&_svg:not([class*='text-'])]:text-current",
          )}
        >
          <X />
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
  const { pending, error, run } = useAction();
  // The row leaves at once; the projects the accept just granted come with the
  // page the action's revalidation draws again.
  const settle = (answer: () => Promise<ActionResult>) =>
    run(answer, () => removeIncomingInvite(invite.id));

  return (
    <div
      className="grid gap-2 border-b border-border px-gutter py-gutter last:border-b-0"
      data-testid="notifications-menu-invite"
    >
      <div className="grid min-w-0 gap-0.5">
        <span className="truncate text-sm font-medium">
          {invite.targetName}
        </span>
        <span className="truncate text-xs text-muted-foreground">
          {inviteSubtitle(invite)}
        </span>
        <span className="truncate text-xs text-muted-foreground">
          from {inviterText(invite)}
        </span>
      </div>

      {error && (
        <p className="text-xs text-destructive" role="alert">
          {error}
        </p>
      )}

      <div className="flex items-center gap-gutter">
        <Button
          size="sm"
          className="flex-1 gap-1"
          disabled={pending}
          onClick={() => settle(() => acceptIncomingInviteAction(invite.id))}
        >
          <Check />
          Accept
        </Button>
        {/* No confirmation step here, unlike the page's table. A dropdown that
            opens a dialog over itself loses the menu underneath, and declining
            is recoverable — an owner or admin can invite again. */}
        <Button
          variant="outline"
          size="sm"
          className="flex-1 gap-1 text-muted-foreground hover:text-foreground"
          disabled={pending}
          onClick={() => settle(() => declineIncomingInviteAction(invite.id))}
        >
          <X />
          Decline
        </Button>
      </div>
    </div>
  );
}
