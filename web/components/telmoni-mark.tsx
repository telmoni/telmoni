import { cn } from "@/lib/utils";

/** The tm monogram from `app/icon.svg`, painted from tokens (`--mark-*`,
 *  `globals.css`) so it takes the shades of the surface it sits on. */
export function TelmoniMark({ className }: { className?: string }) {
  return (
    <svg viewBox="40 125 955 740" aria-hidden="true" className={cn("size-5", className)}>
      <rect className="fill-mark-ink" x="145" y="125" width="130" height="740" rx="65" />
      <rect className="fill-mark-ink" x="40" y="335" width="350" height="120" rx="60" />
      <rect className="fill-mark-mid" x="385" y="455" width="130" height="410" rx="65" />
      <path className="fill-mark-mid" d="M385 520A185 185 0 0 1 755 520H625A55 55 0 0 0 515 520Z" />
      <rect className="fill-mark-mid" x="625" y="455" width="130" height="410" rx="65" />
      <path className="fill-mark-faint" d="M625 520A185 185 0 0 1 995 520H865A55 55 0 0 0 755 520Z" />
      <rect className="fill-mark-faint" x="865" y="455" width="130" height="410" rx="65" />
    </svg>
  );
}
