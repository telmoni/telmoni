import { cn } from "@/lib/utils";

/** The chip mark from `app/icon.svg` in colour, as the brand pack's
 *  `mark-color-on-light` and `-on-dark` masters draw it. Its three parts are
 *  painted from tokens (`--mark-*`, `globals.css`) rather than spelled here,
 *  so the chip inverts with the theme it sits on, as the masters do. */
export function TelmoniMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 1024 1024" aria-hidden="true" className={cn("size-5", className)}>
      <path className="fill-mark-chip" d="M320 256H704A256 256 0 0 1 704 768H320A256 256 0 0 1 320 256Z" />
      <path className="fill-mark-light" d="M192 512A128 128 0 1 1 448 512A128 128 0 1 1 192 512Z" />
      <path className="fill-mark-line" d="M640 448H768A64 64 0 0 1 768 576H640A64 64 0 0 1 640 448Z" />
    </svg>
  );
}
