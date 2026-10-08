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
import { Kbd } from "@/components/ui/kbd";
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

const ROW = "h-8 gap-2 px-2 cursor-pointer";

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
            aria-keyshortcuts="Control+K Meta+K"
            title={name}
            size={null}
            className="size-8 shrink-0 rounded-md text-muted-foreground hover:bg-sidebar-accent hover:text-foreground"
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
          sideOffset={14}
        >
          <DropdownMenuLabel className="p-0 font-normal">
            <div className="flex h-12 items-center gap-2 px-2">
              <div className="grid min-w-0 flex-1 leading-tight">
                <span className="truncate text-sm font-medium">{name}</span>
                <span className="truncate text-xs text-muted-foreground">
                  {user.email}
                </span>
              </div>
            </div>
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

            <DropdownMenuItem
              className={ROW}
              onSelect={() =>
                openShortcuts()
              }
            >
              <Keyboard />
              <span className="flex-1">Keyboard shortcuts</span>
              <Kbd>?</Kbd>
            </DropdownMenuItem>

            <DropdownMenuSeparator />
            <div
              data-slot="dropdown-menu-item"
              className="flex h-8 items-center justify-between gap-2 px-2"
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
              className="mt-1 flex h-8 items-center justify-between gap-2 px-2"
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


            {REPO_URL && (
              <DropdownMenuItem asChild className={ROW}>
                <a href={REPO_URL} target="_blank" rel="noreferrer">
                  <GithubIcon />
                  <span>GitHub</span>
                </a>
              </DropdownMenuItem>
            )}
            {STATUS_URL && (
              <DropdownMenuItem asChild className={ROW}>
                <a href={STATUS_URL} target="_blank" rel="noreferrer">
                  <Activity />
                  <span>Status</span>
                </a>
              </DropdownMenuItem>
            )}
          </DropdownMenuGroup>

          {/* The `border-t` is the rule; the negative margins cancel the
              panel's `p-1` so the row reaches the edges. */}
          <DropdownMenuItem
            className="-mx-1 mt-1 -mb-1 h-14 cursor-pointer gap-3 rounded-none border-t px-3"
            onSelect={(event) => {
              event.preventDefault();
              const form = document.createElement("form");
              form.method = "POST";
              form.action = "/auth/logout";
              document.body.appendChild(form);
              form.submit();
            }}
          >
            <LogOut className="size-4 shrink-0" />
            <span className="text-sm font-medium">Sign out</span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  );
}
