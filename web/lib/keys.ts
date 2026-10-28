import {
  NOTIFICATIONS_HREF,
  buildConsoleNav,
  consolePlace,
  type ConsoleNavItem,
} from "./console-nav";
import type { FlagSet } from "./flags";
import { organizationPath } from "./slug";

export const SEARCH_KEY = "/";

// ⚠ **⌘K is the search key everywhere else, so it is the search key here.**
// The account menu uses ⌘⇧K — see `nav-user.tsx`.
export const SEARCH_MODIFIER_KEY = "k";

// What stands in front of the page and takes its keys and its presses: a
// modal dialog, an alert, a menu. One list for every reader — the letter keys,
// the rail's drawer, ⌘J — which must agree about what counts as in front.
// ⚠ **The agent's window is a dialog that does not count**: it floats beside
// the page without shutting it off, and says so with `aria-modal="false"`
// (`components/agent/agent-window.tsx`), so the page keeps its keys while it
// is open. A dialog that shuts the page off never says that.
export const MODAL_SELECTOR =
  '[role="dialog"]:not([aria-modal="false"]), [role="alertdialog"], [role="menu"]';

// ⌘J, beside ⌘K: the two are the console's ways to ask it something, and a
// hand on one finds the other.
export const AGENT_MODIFIER_KEY = "j";

// What a keydown says was pressed, or "" for one that says nothing. A
// password manager or another extension dispatches a plain Event under the
// keydown name, with no `key` on it at all, and `e.key.toLowerCase()` in a
// document-level handler threw on one. A KeyboardEvent never says less than
// "", so that is what such an event reads as, and every chord returns.
export function pressedKey(e: KeyboardEvent): string {
  return typeof e.key === "string" ? e.key : "";
}

// What types its own keys: a field, a list, and a form in a panel, whose
// switches and draft are its own.
const TYPING_SELECTOR =
  'input, textarea, select, [contenteditable="true"], [role="combobox"], [role="listbox"], [role="dialog"] form';

/** Whether a key went to something that types, or to an input method still composing. */
export function typedInto(e: KeyboardEvent): boolean {
  return e.isComposing || (e.target instanceof Element && e.target.closest(TYPING_SELECTOR) !== null);
}

/** Whether a modal dialog, an alert or a menu stands in front of the page. */
export function inFront(): boolean {
  return document.querySelector(MODAL_SELECTOR) !== null;
}

/** Whether `e` is ⌘ or Ctrl with `key`, Shift as asked, and no page answered it first. */
export function isChord(e: KeyboardEvent, key: string, shift = false): boolean {
  return (
    !e.defaultPrevented &&
    !e.altKey &&
    (e.ctrlKey || e.metaKey) &&
    e.shiftKey === shift &&
    pressedKey(e).toLowerCase() === key
  );
}

export const SEQUENCE_TIMEOUT_MS = 1000;

export const PAGE_ACTION_KEY = "c";

/** Step to the next and the previous of what a page steps through
 *  (`lib/use-page-steps.ts`): the trace list's peek through the page's runs,
 *  and a session's page through its traces, as a mail reader's j and k do;
 *  `o` opens the one stood on, where the page offers that. `k` is also the
 *  API keys row's `g k` and `o` the Overview's `g o`, and the pending-`g`
 *  branch takes both first, as it does `c`. */
export const NEXT_KEY = "j";
export const PREVIOUS_KEY = "k";
export const OPEN_KEY = "o";

/** `g` then this reaches the organization's overview from anywhere, Account
 *  included. */
export const ORGANIZATION_KEY = "O";

/** `g` then this reaches Notifications, the bell's page, from anywhere in the
 *  console. */
export const NOTIFICATIONS_KEY = "n";

export interface GoSequence {
  key: string;
  label: string;
  href: string;
}

// The `g` sequences are the rail's rows: a row with a `key` in
// `lib/console-nav.ts`, or one a console built on this one adds through
// `lib/extension/nav.ts`, is reached by `g` and that key the day it lands,
// and the sheet lists exactly the rail you stand in, so no sequence promises
// a page the rail lacks; only `g O` and `g n` reach their page from anywhere.
// `c` is also `PAGE_ACTION_KEY`, safely: the pending-`g` branch returns
// before the page-action check, so the two readings never meet.
//
// ⚠ **A sequence with nowhere to go must do nothing.** A bare project-relative
// href — `/api-keys` — reads as an organization of that name and 404s. Off
// the console's paths the rail is empty, so nothing resolves there but `g O`
// with an organization to go to.
export function goSequences(
  pathname: string,
  organization?: string | null,
  flags: FlagSet = {},
): GoSequence[] {
  const rows = buildConsoleNav(pathname, flags)
    .flatMap((g) => g.items)
    .filter((item): item is ConsoleNavItem & { key: string } => item.key !== undefined)
    .map((item) => ({ key: item.key, label: item.title, href: item.url }));
  const place = consolePlace(pathname);
  if (place !== null && !rows.some((s) => s.key === NOTIFICATIONS_KEY)) {
    rows.push({ key: NOTIFICATIONS_KEY, label: "Notifications", href: NOTIFICATIONS_HREF });
  }
  const slug = place !== null && place.kind !== "account" ? place.organization : organization;
  if (slug) {
    rows.push({
      key: ORGANIZATION_KEY,
      label: "The organization's overview",
      href: organizationPath(slug),
    });
  }
  return rows;
}

/** A sequence as a tooltip names it, in the shortcut sheet's own keys. */
export function sequenceHint(key: string): string {
  return `G then ${/^[A-Z]$/.test(key) ? `⇧ ${key}` : key.toUpperCase()}`;
}

/** Where `g` and `key` go from `pathname`; `null` is nowhere, and nothing. */
export function resolveSequence(
  key: string,
  pathname: string,
  organization?: string | null,
  flags: FlagSet = {},
): string | null {
  return goSequences(pathname, organization, flags).find((s) => s.key === key)?.href ?? null;
}
