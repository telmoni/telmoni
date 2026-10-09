"use client";

import { useConsoleUi } from "@/components/console-ui-context";

import { useEffect, useState } from "react";
import Link from "next/link";
import { useTheme } from "next-themes";
import {
  Activity,
  BookOpen,
  House,
  Keyboard,
  LogOut,
  Monitor,
  Moon,
  Palette,
  Square,
  SquareRoundCorner,
  Sun,
  User,
} from "lucide-react";

import { GithubIcon } from "@/components/brand-icons";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { CORNERS_DEFAULT, type Corners } from "@/lib/corners";
import { setCorners, useCorners } from "@/lib/use-corners";
import { ACCOUNT_MENU_EVENT } from "@/lib/keys";
import { DOCS_URL, REPO_URL, STATUS_URL } from "@/lib/site";
import { useUser } from "@/lib/store";
import { displayName } from "@/lib/user-display";
import { cn } from "@/lib/utils";

const THEMES = [
  { value: "system", label: "System theme", icon: Monitor },
  { value: "light", label: "Light theme", icon: Sun },
  { value: "dark", label: "Dark theme", icon: Moon },
] as const;

const ROW = "h-9 gap-2 px-2 cursor-pointer";

const CORNER_CHOICES = [
  { value: "rounded", label: "Rounded corners", icon: SquareRoundCorner },
  { value: "sharp", label: "Sharp corners", icon: Square },
] as const satisfies ReadonlyArray<{
  value: Corners;
  label: string;
  icon: typeof Square;
}>;

// `SegmentedSwitch`'s shape, built from Radix menu items instead of buttons;
// its track classes are repeated on the two groups below.
const SWITCH_SEGMENT = [
  "size-7 justify-center gap-0 p-0 pl-0 rounded-none",
  "[&>span:first-child]:hidden",
  "text-muted-foreground data-[state=checked]:bg-secondary",
  "data-[state=checked]:text-foreground",
];

export function NavUser() {
  const user = useUser();
  const { theme, setTheme } = useTheme();
  const corners = useCorners();
  const { openShortcuts } = useConsoleUi();
  const [open, setOpen] = useState(false);

  useEffect(() => {
    const onAsk = () => setOpen(true);
    window.addEventListener(ACCOUNT_MENU_EVENT, onAsk);
    return () => window.removeEventListener(ACCOUNT_MENU_EVENT, onAsk);
  }, []);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() !== "k" || !(e.ctrlKey || e.metaKey)) return;
      if (e.defaultPrevented || e.altKey || !e.shiftKey) return;
      if (
        document.querySelector(
          '[data-slot="dialog-content"], [data-slot="alert-dialog-content"], [data-slot="sheet-content"]',
        )
      ) {
        return;
      }
      e.preventDefault();
      setOpen((v) => !v);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  if (!user) return null;

  const name = displayName(user);

  return (
    <>
      <DropdownMenu modal={false} open={open} onOpenChange={setOpen}>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            aria-label={`Account: ${name}`}
            aria-keyshortcuts="Control+Shift+K Meta+Shift+K"
            title={name}
            size={null}
            className="size-9 shrink-0 rounded-md text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground"
          >
            <Avatar>
              <AvatarImage src="" alt="" />
              <AvatarFallback className="bg-transparent text-inherit">
                <User aria-hidden className="size-4" />
              </AvatarFallback>
            </Avatar>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          className="w-64 animate-none!"
          side="bottom"
          align="end"
          // Right under the header's rule, as the switchers' menus open
          // (`resource-selector.tsx`).
          sideOffset={12.5}
        >
          {/* The head is the address alone, a row the foot's height, as the
              switchers head their menus with one line: the name is the
              account page's to show, not the menu's. */}
          <DropdownMenuLabel className="flex h-9 items-center px-2 py-0 text-xs font-normal text-muted-foreground">
            <span className="truncate">{user.email}</span>
          </DropdownMenuLabel>

          <DropdownMenuSeparator />
          <DropdownMenuGroup>
            <DropdownMenuItem asChild className={ROW}>
              <Link href="/account/settings">
                <User />
                <span>Account</span>
              </Link>
            </DropdownMenuItem>

            <DropdownMenuItem asChild className={ROW}>
              <Link href="/">
                <House />
                <span>Home page</span>
              </Link>
            </DropdownMenuItem>

            <DropdownMenuItem asChild className={ROW}>
              <a href={DOCS_URL} target="_blank" rel="noreferrer">
                <BookOpen />
                <span>Docs</span>
              </a>
            </DropdownMenuItem>

            {/* Beside Docs, the other way out to what is written down. */}
            {REPO_URL && (
              <DropdownMenuItem asChild className={ROW}>
                <a href={REPO_URL} target="_blank" rel="noreferrer">
                  <GithubIcon />
                  <span>GitHub</span>
                </a>
              </DropdownMenuItem>
            )}

            <DropdownMenuItem
              className={ROW}
              onSelect={() =>
                openShortcuts()
              }
            >
              <Keyboard />
              <span className="flex-1">Keyboard shortcuts</span>
              {/* The `?` it is, as the sheet lists it: the keys that type one
                  differ from keyboard to keyboard. Hidden while letter-key
                  shortcuts are off, when it does nothing. */}
              <KbdGroup data-letter-key="">
                <Kbd>?</Kbd>
              </KbdGroup>
            </DropdownMenuItem>

            <DropdownMenuSeparator />
            <div
              data-slot="dropdown-menu-item"
              className="flex h-9 items-center justify-between gap-2 px-2"
            >
              <span className="flex items-center gap-2 text-sm text-muted-foreground">
                <Palette className="size-4" />
                Theme
              </span>
              <DropdownMenuRadioGroup
                value={theme ?? "system"}
                onValueChange={setTheme}
                aria-label="Theme"
                className="flex items-center overflow-hidden rounded-md border border-border"
              >
                {THEMES.map(({ value, label, icon: Icon }) => (
                  <DropdownMenuRadioItem
                    key={value}
                    value={value}
                    aria-label={label}
                    title={label}
                    onSelect={(e) => e.preventDefault()}
                    className={cn(SWITCH_SEGMENT)}
                  >
                    <Icon className="size-3.5" />
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
            </div>

            <div
              data-slot="dropdown-menu-item"
              className="mt-0.5 flex h-9 items-center justify-between gap-2 px-2"
            >
              <span className="flex items-center gap-2 text-sm text-muted-foreground">
                <SquareRoundCorner className="size-4" />
                Corners
              </span>
              <DropdownMenuRadioGroup
                value={corners ?? CORNERS_DEFAULT}
                onValueChange={(v) => setCorners(v as Corners)}
                aria-label="Corners"
                className="flex items-center overflow-hidden rounded-md border border-border"
              >
                {CORNER_CHOICES.map(({ value, label, icon: Icon }) => (
                  <DropdownMenuRadioItem
                    key={value}
                    value={value}
                    aria-label={label}
                    title={label}
                    onSelect={(e) => e.preventDefault()}
                    className={cn(SWITCH_SEGMENT)}
                  >
                    <Icon className="size-3.5" />
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
            </div>


            {STATUS_URL && (
              <DropdownMenuItem asChild className={ROW}>
                <a href={STATUS_URL} target="_blank" rel="noreferrer">
                  <Activity />
                  <span>Status</span>
                </a>
              </DropdownMenuItem>
            )}
          </DropdownMenuGroup>

          {/* The foot is the switchers' foot: a rule, then one 36px row, so
              every menu in the header ends the same way. */}
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className="h-9 cursor-pointer gap-2 px-2"
            onSelect={(event) => {
              event.preventDefault();
              const form = document.createElement("form");
              form.method = "POST";
              form.action = "/auth/logout";
              document.body.appendChild(form);
              form.submit();
            }}
          >
            <LogOut className="size-4 shrink-0 text-muted-foreground" />
            Sign out
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  );
}
