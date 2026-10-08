import { cn } from "@/lib/utils";

/** The pixel mark from `app/icon.svg`, drawn in `currentColor` so it takes
 *  its ink from whatever it sits on. */
export function TelmoniMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 100 100" fill="none" aria-hidden="true" className={cn("size-5", className)}>
      <path d="M10 44 V90 H90 V44" stroke="currentColor" strokeWidth="8" />
      <rect x="38" y="58" width="24" height="24" fill="currentColor" />
      <rect x="44" y="8" width="12" height="12" fill="currentColor" />
      <rect x="44" y="26" width="12" height="12" fill="currentColor" />
    </svg>
  );
}
