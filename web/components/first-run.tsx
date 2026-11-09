import { Check } from "lucide-react";
import type { ReactNode } from "react";

import { StepNumber } from "@/components/ui/step-number";

// A page's first run: what it shows once something lands, and the steps to get
// there, drawn by the server in place of an empty list, so nothing flashes first.

/** The page's first run, its steps numbered under its title. */
export function FirstRun({
  title,
  lead,
  status,
  media,
  children,
}: {
  title: string;
  lead: ReactNode;
  /** A live line above the title, such as the page listening for its first run. */
  status?: ReactNode;
  /** A picture or a video of the page once it has something, beside the steps. */
  media?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section aria-labelledby="first-run" className="grid gap-gutter md:max-w-3xl">
      {status}
      <div>
        <h2 id="first-run" className="text-base font-medium">
          {title}
        </h2>
        <p className="text-sm text-muted-foreground">{lead}</p>
      </div>
      <ol className="grid gap-gutter">{children}</ol>
      {media}
    </section>
  );
}

/** One step: its number, or a tick once done, its title, a line on it, and what it offers. */
export function FirstRunStep({
  n,
  title,
  note,
  done = false,
  children,
}: {
  n: number;
  title: string;
  note: ReactNode;
  done?: boolean;
  children?: ReactNode;
}) {
  return (
    <li className="grid grid-cols-[auto_minmax(0,1fr)] gap-gutter rounded-md border border-border p-gutter">
      <StepNumber aria-hidden="true">
        {done ? <Check className="size-icon text-brand-positive" /> : n}
      </StepNumber>
      <div className="grid min-w-0 gap-gutter">
        <div>
          <h3 className="text-sm font-medium">
            {title}
            {done && <span className="sr-only"> (done)</span>}
          </h3>
          <p className="text-sm text-muted-foreground">{note}</p>
        </div>
        {children}
      </div>
    </li>
  );
}

/** A live line: the page is waiting for something to land, and redraws itself when it does. */
export function Listening({ children }: { children: ReactNode }) {
  return (
    <p role="status" className="flex w-fit items-center gap-gutter text-sm text-muted-foreground">
      <span className="relative flex size-2" aria-hidden="true">
        <span className="absolute inline-flex size-full rounded-full bg-brand-positive opacity-75 motion-safe:animate-ping" />
        <span className="relative inline-flex size-2 rounded-full bg-brand-positive" />
      </span>
      {children}
    </p>
  );
}
