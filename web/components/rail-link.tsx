import Link from "next/link";
import { ArrowUpRight } from "lucide-react";

import { StatusLabel } from "@/components/status-label";
import type { FooterLink } from "@/lib/footer";
import type { NavLink } from "@/components/site-nav";
import { cn } from "@/lib/utils";

/** A link of the public rail, and of its menu on a phone: muted text that
 *  turns the text colour, a new tab's marked by an arrow. The rail draws the
 *  arrow on hover or focus; a phone, which has no hover, always (`arrow`).
 *  `onDone` hears a navigation in this tab, or any click on a new tab's. A
 *  Route Handler such as `/auth/login` is a plain anchor (`handler`): a
 *  `<Link>`'s prefetch would run it, starting a sign-in nobody asked for. */
export function RailLink({
  link,
  className,
  arrow = "hover",
  handler = false,
  onDone,
}: {
  link: NavLink | FooterLink;
  className?: string;
  arrow?: "hover" | "always";
  handler?: boolean;
  onDone?: () => void;
}) {
  // An address to write to opens the mail app, in no tab at all.
  const mail = link.href.startsWith("mailto:");
  const leaves = link.newTab || (!link.href.startsWith("/") && !mail);
  const content = (
    <>
      <StatusLabel label={link.label} services={"services" in link ? link.services : undefined} />
      {leaves && (
        <ArrowUpRight
          aria-hidden="true"
          className={cn(
            "ml-1 inline-block size-icon align-[-0.125em]",
            arrow === "hover" &&
              "opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100 motion-reduce:transition-none",
          )}
        />
      )}
    </>
  );
  const look = cn("group text-muted-foreground transition-colors hover:text-foreground", className);
  if (handler || mail) {
    return (
      <a href={link.href} onClick={onDone} className={look}>
        {content}
      </a>
    );
  }
  if (link.href.startsWith("/")) {
    return (
      <Link
        href={link.href}
        {...(link.newTab
          ? { target: "_blank", rel: "noreferrer", onClick: onDone }
          : { onNavigate: onDone })}
        className={look}
      >
        {content}
      </Link>
    );
  }
  return (
    <a href={link.href} target="_blank" rel="noreferrer" onClick={onDone} className={look}>
      {content}
    </a>
  );
}
