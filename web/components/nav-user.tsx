"use client";

import { useConsoleUi } from "@/components/console-ui-context";

import { useEffect, useState } from "react";
import Link from "next/link";
import { Activity, BookOpen, House, Keyboard, LogOut, Settings, User } from "lucide-react";

import { GithubIcon } from "@/components/brand-icons";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { signOut } from "@/lib/auth/sign-out";
import { SEARCH_MODIFIER_KEY, inFront, isChord } from "@/lib/keys";
import { useIsMac } from "@/lib/use-platform";
import { DOCS_URL, REPO_URL, STATUS_URL } from "@/lib/site";
import { useUser } from "@/lib/store";
import { displayName } from "@/lib/user-display";
import { cn } from "@/lib/utils";

const ROW = "h-7 gap-2 px-2 cursor-pointer";

export function NavUser() {
  const user = useUser();
  const { setShortcutsOpen } = useConsoleUi();
  const [open, setOpen] = useState(false);
  const isMac = useIsMac();

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!isChord(e, SEARCH_MODIFIER_KEY, true) || inFront()) return;
      e.preventDefault();
      setOpen((v) => !v);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  if (!user) return null;

  const name = displayName(user);
  const chord = isMac ? "⌘⇧K" : "Ctrl Shift K";

  return (
    <>
      <DropdownMenu modal={false} open={open} onOpenChange={setOpen}>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            aria-label={`Account: ${name}`}
            aria-keyshortcuts="Control+Shift+K Meta+Shift+K"
            title={`${name} (${chord})`}
            size={null}
            className="size-7 shrink-0 rounded-md text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground"
          >
            <Avatar className="size-full">
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
          sideOffset={12}
        >
          {/* The head is the address alone, a row the foot's height, as the
              switchers head their menus with one line: the name is the
              account page's to show, not the menu's. */}
          <DropdownMenuLabel className="flex h-7 items-center px-2 py-0 text-xs font-normal text-muted-foreground">
            <span className="truncate">{user.email}</span>
          </DropdownMenuLabel>

          <DropdownMenuSeparator />
          <DropdownMenuGroup>
            <DropdownMenuItem asChild className={ROW}>
              <Link href="/settings">
                <Settings />
                <span>Settings</span>
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
              className={cn(ROW, "pr-1")}
              onSelect={() => setShortcutsOpen(true)}
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

            {STATUS_URL && (
              <DropdownMenuItem asChild className={ROW}>
                <a href={STATUS_URL} target="_blank" rel="noreferrer">
                  <Activity />
                  <span>Status</span>
                </a>
              </DropdownMenuItem>
            )}
          </DropdownMenuGroup>

          {/* The foot is the switchers' foot: a rule, then one 28px row, so
              every menu in the header ends the same way. */}
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className="h-7 cursor-pointer gap-2 px-2"
            onSelect={(event) => {
              event.preventDefault();
              signOut();
            }}
          >
            <LogOut />
            Sign out
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  );
}
