"use client";

import Link from "next/link";
import { ChevronRight } from "lucide-react";
import { NavigationMenu } from "radix-ui";

import { PRIMARY_NAV, type NavLink, type NavPanel } from "@/components/site-nav";
import { cn } from "@/lib/utils";

// ⚠ **A trigger and a plain link have to be the same shape, so neither is a
// `Button`.** A Button would give the link its pill and the trigger nothing,
// and the two sit side by side in one row. This is `size="sm"`'s geometry
// written out once — `h-9 px-3` — with the ghost variant's fill.
//
// `data-[state=open]` is what borrows the open look: the item whose panel is
// showing stays filled, so the row says which panel you are reading. Radix
// sets it, and there is no chevron to turn because the fill already says it.
const ROW = cn(
  "inline-flex h-9 cursor-pointer items-center rounded-md px-3",
  "text-sm font-normal whitespace-nowrap transition-colors",
  "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
  "data-[state=open]:bg-accent data-[state=open]:text-accent-foreground",
  "outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50",
);

// ⚠ **No `NavigationMenu.Viewport`, and the shrinking panel is why.** A
// Viewport sizes itself from a measurement of whichever panel is open, exposed
// as `--radix-navigation-menu-viewport-width`, and clips whatever overflows.
// Pair that with a width transition and a panel whose own width depends on the
// window and you get exactly what it looked like: the box narrowing while the
// content inside it stayed 44rem, so the right-hand column was cut off
// mid-animation. A Viewport earns that complexity when panels differ in size
// and you want one box to morph between them. These are one stated width, so
// each panel positions itself and there is nothing to measure.
//
// ⚠ **A card: bordered and curved on all four sides, sat one pixel clear of
// the bar.** The bar carries no rule, so the panel draws its own top edge, and
// `mt-px` puts that edge exactly where a 1px `border-b` would have been — the
// panel hangs from the line rather than from the bar. That single pixel is
// also what earns the top radius: flush, the two upper corners would have
// shown the page through them. `rounded-menu` keeps all four inside the
// sharp-corner setting.
//
// ⚠ **`absolute` resolves against the ITEM, which is why the item is the
// positioned one.** Anchored to the Root instead, every panel opened at the
// left edge of the list, so `Company`'s hung four items away from the word
// that opened it and looked unrelated to it. `left-0` now lands on its own
// trigger's left edge.
//
function outbound(link: NavLink) {
  return link.newTab ? { target: "_blank", rel: "noreferrer" } : {};
}

function Panel({ panel }: { panel: NavPanel }) {
  const wide = panel.features.length > 1;
  const manyGroups = (panel.groups?.length ?? 0) > 1;
  return (
    // ⚠ **Sized for the LAST trigger in the row, not the first.** A panel
    // anchored to its own trigger starts further right the further right the
    // word is, and `Company` is the rightmost one that has a panel.
    //
    // Its offset does not move with the window: everything left of it — the
    // gutter, the wordmark, the gaps, three fixed-width items — is content, so
    // the trigger opens ~465px in at any width. At 1024px, the narrowest this
    // row appears at, that leaves ~530px before the gutter and the reserved
    // scrollbar. 30rem clears it. The clamp is the backstop.
    //
    // That offset shrank when the row went to the console's `gap-3`; it grows
    // again if the gaps or the wordmark do, so re-measure before widening.
    //
    // Widening this means measuring where `Company` actually starts at 1024px
    // first — the failure is silent, a panel running off the right of the
    // window with no scrollbar to say so.
    <div className="w-[30rem] max-w-[calc(100vw-2rem)]">
      {/* `divide-x` rather than a border per child: the rule belongs between
          two features, and a child that draws its own has to know whether it
          is last. */}
      <div className={cn("grid divide-border", wide && "sm:grid-cols-2 sm:divide-x")}>
        {panel.features.map((feature) => (
          <NavigationMenu.Link asChild key={feature.href}>
            <Link
              href={feature.href}
              {...outbound(feature)}
              className="flex flex-col gap-1.5 p-6 transition-colors hover:bg-accent"
            >
              <span className="flex items-center gap-1.5 text-base text-foreground">
                {feature.label}
                <ChevronRight className="size-3.5 text-muted-foreground" />
              </span>
              <span className="text-sm text-muted-foreground">{feature.detail}</span>
            </Link>
          </NavigationMenu.Link>
        ))}
      </div>

      {panel.groups && panel.groups.length > 0 && (
        <div
          className={cn(
            "grid divide-border border-t border-border",
            manyGroups && "sm:grid-cols-2 sm:divide-x",
          )}
        >
          {panel.groups.map((group) => (
            <div key={group.title} className="flex flex-col gap-3 p-6">
              <span className="text-xs tracking-label text-muted-foreground uppercase">
                {group.title}
              </span>
              {group.links.map((link) => (
                <NavigationMenu.Link asChild key={link.href}>
                  <Link
                    href={link.href}
                    {...outbound(link)}
                    className="text-sm text-foreground transition-colors hover:text-muted-foreground"
                  >
                    {link.label}
                  </Link>
                </NavigationMenu.Link>
              ))}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export function SiteNavMenu() {
  return (
    <NavigationMenu.Root
      className="hidden items-center self-stretch lg:flex"
      delayDuration={100}
    >
      <NavigationMenu.List
        className="flex h-9 list-none items-center gap-3"
      >
        {PRIMARY_NAV.map((item) => (
          <NavigationMenu.Item
            key={item.label}
            className="relative flex h-9 items-center"
          >
            {item.panel ? (
              <>
                <NavigationMenu.Trigger className={ROW}>
                  {item.label}
                </NavigationMenu.Trigger>
                {/* `top-full` is the 32px item's bottom; `mt-3.5` is the
                    header's `py-3.5` below it, so the panel opens on the
                    header's bottom edge. */}
                <NavigationMenu.Content className="absolute top-full left-0 z-50 mt-3.5 rounded-menu border border-border bg-popover data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0">
                  <Panel panel={item.panel} />
                </NavigationMenu.Content>
              </>
            ) : (
              <NavigationMenu.Link asChild>
                <Link href={item.href} {...outbound(item)} className={ROW}>
                  {item.label}
                </Link>
              </NavigationMenu.Link>
            )}
          </NavigationMenu.Item>
        ))}
      </NavigationMenu.List>

    </NavigationMenu.Root>
  );
}
