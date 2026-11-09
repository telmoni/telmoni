"use client";

import { useConsoleUi } from "@/components/console-ui-context";

import { useEffect, useState } from "react";
import Link from "next/link";
import { Activity, BookOpen, House, Keyboard, LogOut, Settings, User } from "lucide-react";

import { GithubIcon } from "@/components/brand-icons";
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
import { signOut } from "@/lib/auth/sign-out";
import { SEARCH_MODIFIER_KEY, inFront, isChord } from "@/lib/keys";
import { useIsMac } from "@/lib/use-platform";
import { DOCS_URL, REPO_URL, STATUS_URL } from "@/lib/site";
import { useUser } from "@/lib/store";
import { displayName } from "@/lib/user-display";
import { HEADER_MENU_OFFSET } from "@/lib/utils";

const ROW = "cursor-pointer";

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
            size="icon"
            className="shrink-0 rounded-md text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground data-[state=open]:bg-sidebar-accent data-[state=open]:text-foreground data-[state=open]:hover:bg-sidebar-accent"
          >
            <User aria-hidden className="size-icon" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          className="w-64 animate-none!"
          side="bottom"
          align="end"
          sideOffset={HEADER_MENU_OFFSET}
        >
          {/* The head is the address alone, a row the foot's height, as the
              switchers head their menus with one line: the name is the
              account page's to show, not the menu's. */}
          <DropdownMenuLabel className="flex h-control items-center py-0">
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

            <DropdownMenuItem className={ROW} onSelect={() => setShortcutsOpen(true)}>
              <Keyboard />
              <span>Keyboard shortcuts</span>
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

          {/* Every header menu ends with the same ruled action row. */}
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className="cursor-pointer"
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
