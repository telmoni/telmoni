"use client";

import { type KeyboardEvent } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { ArrowLeft } from "lucide-react";

import { ScrollArea } from "@/components/ui/scroll-area";
import {
  buildConsoleNav,
  consolePlace,
  navBlocks,
  type ConsoleNavItem,
} from "@/lib/console-nav";
import { resolveReturnUrl } from "@/lib/console-trail";
import { PRODUCT_NAME } from "@/lib/site";
import { useFlags } from "@/lib/store";
import { useConsoleTrail, useLiveResources } from "@/lib/use-console-trail";
import { cn } from "@/lib/utils";
import { useSidebar } from "./sidebar-context";

function NavRow({
  item,
  showLabel,
  step = false,
  className,
}: {
  item: ConsoleNavItem;
  showLabel: boolean;
  step?: boolean;
  className?: string;
}) {
  const onClick = () => {
    if (item.title && !step) {
      document.title = `${item.title} · ${PRODUCT_NAME}`;
    }
  };
  // A row is the docs sidebar's (Fumadocs, `/docs`): 36px tall, 2px apart, 8px
  // corners, muted at rest, the sidebar accent at half under the pointer, and
  // the primary colour at a tenth behind primary text when it is where you
  // are. Closed, it is a 36px square in a column 12px in on each side; open,
  // its icon keeps a 32px box at the row's left edge, 14px in. Both put the
  // icon's centre 30px from the window's edge, where the header's mark
  // stands, at the same height row for row, so nothing slides when the rail
  // toggles — Gemini's rail is the reference; the label then starts where the
  // header's name does.
  if (!showLabel) {
    return (
      <Link
        href={item.url}
        onClick={onClick}
        aria-label={item.title}
        title={item.title}
        aria-current={item.isActive ? "page" : undefined}
        {...(step ? { ["data-rail-step"]: "" } : {})}
        className={cn(
          "flex size-9 items-center justify-center rounded-md [a+&]:mt-0.5",
          item.isActive
            ? "bg-primary/10 text-primary"
            : "text-muted-foreground hover:bg-sidebar-accent/50 hover:text-sidebar-accent-foreground/80",
          className,
        )}
      >
        <item.icon className="size-4" />
      </Link>
    );
  }
  const rowClass = cn(
    "flex h-9 items-center gap-0.5 rounded-md text-sm [a+&]:mt-0.5",
    item.isActive
      ? "bg-primary/10 text-primary"
      : "text-muted-foreground hover:bg-sidebar-accent/50 hover:text-sidebar-accent-foreground/80",
    className,
  );
  const icon = (
    <span className="flex w-8 shrink-0 justify-center">
      <item.icon
        className="size-4"
      />
    </span>
  );
  const label = (
    <span className="flex flex-1 items-center min-w-0 overflow-hidden pr-1">
      <span className="whitespace-nowrap truncate">{item.title}</span>
    </span>
  );
  return (
    <Link
      href={item.url}
      onClick={onClick}
      aria-current={item.isActive ? "page" : undefined}
      {...(step ? { ["data-rail-step"]: "" } : {})}
      className={rowClass}
    >
      {icon}
      {label}
    </Link>
  );
}

function onNavKeyDown(e: KeyboardEvent<HTMLElement>) {
  if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
  if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
  // Only the rows on screen: the ones the not-found mark hides are still in
  // the document, and an arrow key must not land on one.
  const rows = Array.from(
    e.currentTarget.querySelectorAll<HTMLElement>("a[href]"),
  ).filter((row) => row.getClientRects().length > 0);
  const from = (e.target as HTMLElement).closest<HTMLElement>("a[href]");
  const at = from ? rows.indexOf(from) : -1;
  if (at === -1) return;
  e.preventDefault();
  const to =
    e.key === "ArrowDown"
      ? Math.min(at + 1, rows.length - 1)
      : e.key === "ArrowUp"
        ? Math.max(at - 1, 0)
        : e.key === "Home"
          ? 0
          : rows.length - 1;
  rows[to]?.focus();
}

export function ConsoleSidebar() {
  const pathname = usePathname();
  const { collapsed, toggle, closeOnMobile, animating } = useSidebar();
  const expanded = !collapsed;

  const trail = useConsoleTrail();

  const flags = useFlags();
  const groups = buildConsoleNav(pathname, flags);
  const steppedIn = groups[0]?.title === "Account";
  const { live, fallback } = useLiveResources();
  const returnUrl = resolveReturnUrl(trail, live, fallback);
  const returnLabel =
    consolePlace(returnUrl)?.kind === "organization"
      ? "Back to organization"
      : "Back to project";

  // ⚠ **A page that is not found has no rail of its own.** The rail is spelled
  // from the path, and a path that names nothing spells a rail of links to
  // more "not found". `(app)/not-found.tsx` marks itself, and every row here
  // answers the mark in CSS: the address's rows go, and the way back, which
  // Account draws anyway, comes. In CSS because only the page knows — an
  // outage draws the same empty store a dead address does — and the page
  // renders after the rail: a class is right in the server's HTML, where
  // state would correct itself once the console had hydrated.
  //
  // The rail is a column under the header's first cell: the same width, and
  // from `md` the same rule on its right, flush with the window's edge. Open,
  // its rows keep the `px-3.5` inset; closed, the column is one 36px square
  // 12px in on each side (`px-3`), so every icon lands on the centre the
  // header's mark stands on. The control that opens and closes it is the
  // header's, in the cell above; closed, the whole column is a way to open
  // it too — the cursor says so, a bar and an arrow (`cursor-expand`) as
  // Gemini draws it, and a press anywhere off a row opens it —
  // so nobody has to aim at the one button. Below `md` it is a drawer under
  // the header, opened from the header's hamburger.
  return (
    <aside
      id="console-sidebar"
      className={cn(
        "flex min-h-0 flex-col overflow-hidden bg-sidebar",
        "fixed top-15 bottom-0 left-0 z-40 w-72",
        // The drawer's edge, open only: closed, it waits just off the
        // screen's left edge, where a shadow cast right would show. Closed
        // below `md` it is hidden as well, or Tab would walk its rows unseen;
        // the visibility moves with the slide, so it goes once the slide ends.
        collapsed
          ? "-translate-x-full max-md:invisible md:cursor-expand"
          : "translate-x-0 shadow-drawer md:shadow-none",
        "md:static md:shrink-0 md:translate-x-0 md:border-r md:border-sidebar-border",
        collapsed ? "md:w-15" : "md:w-64",
        // `translate`, the property Tailwind's `-translate-x-full` sets, not
        // `transform`; `visibility` beside it holds the drawer visible until
        // the slide ends.
        animating
          ? "transition-[translate,visibility] duration-200 md:transition-none"
          : "transition-none",
      )}
      onClick={(e) => {
        if ((e.target as HTMLElement).closest("a[href]")) {
          closeOnMobile();
        } else if (collapsed) {
          toggle();
        }
      }}
      aria-label="Console sidebar"
    >
      <ScrollArea
        type="scroll"
        className={cn(
          "min-h-0 flex-1 **:data-[slot=scroll-area-thumb]:bg-muted-foreground/30",
          collapsed && "**:data-[slot=scroll-area-scrollbar]:hidden",
        )}
      >
        <nav
          aria-label="Main"
          onKeyDown={onNavKeyDown}
          className={cn("py-3.5", expanded ? "px-3.5" : "px-3")}
        >
          {groups.map((group, i) => (
            <div
              key={group.title}
              className={cn(expanded && i > 0 && "mt-6")}
            >
              {/* The rows come in blocks (`navBlocks`). An unheaded block
                  after the first stands 24px clear of the one above; a
                  headed one takes the shape sidebars commonly give a
                  section — 16px clear of the rows above, its heading a row of
                  its own, 32px with the label centred, and its first row
                  straight under — so the label sits closer to its rows than to
                  the block before, which is what groups them. The label stands
                  on the icons' left edge (`w-8` centres a 16px icon at 16px,
                  so its edge is 8px in).
                  Closed, a rule stands between blocks instead. A project's
                  Observability and Project blocks are headed, and an
                  organization's own rows are headed Organization under its
                  Overview; Account's rail is one unheaded block, the way back
                  under it saying what it is. A page that is not found hides
                  every block but the first, as it hides the rows, so no gap
                  stands over the way back. */}
              {navBlocks(group.items).map((block, k) => (
                <div
                  key={k}
                  className={cn(
                    k > 0 && expanded && (block.heading ? "mt-4" : "mt-6"),
                    k > 0 && "group-has-[[data-console-not-found]]/console:hidden",
                  )}
                >
                  {expanded ? (
                    block.heading && (
                      <div className="flex h-8 items-center pl-2 text-xs font-medium text-muted-foreground group-has-[[data-console-not-found]]/console:hidden">
                        <span className="whitespace-nowrap">{block.heading}</span>
                      </div>
                    )
                  ) : (
                    k > 0 && <div className="my-2 border-t border-sidebar-border" />
                  )}
                  {block.items.map((item) => (
                    <NavRow
                      key={item.title}
                      item={item}
                      showLabel={expanded}
                      className="group-has-[[data-console-not-found]]/console:hidden"
                    />
                  ))}
                </div>
              ))}
              <NavRow
                item={{ title: returnLabel, url: returnUrl, icon: ArrowLeft, isActive: false }}
                showLabel={expanded}
                step
                className={cn(
                  "mt-0.5",
                  !steppedIn && "hidden group-has-[[data-console-not-found]]/console:flex",
                )}
              />
            </div>
          ))}
        </nav>
      </ScrollArea>
    </aside>
  );
}
