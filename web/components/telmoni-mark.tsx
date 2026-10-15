import { cn } from "@/lib/utils";

/** The chip mark from `app/icon.svg` in one colour: `currentColor`, so it
 *  takes its ink from whatever it sits on, with the live light and the data
 *  line cut out rather than painted, so either theme's surface shows through. */
export function TelmoniMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 1024 1024" aria-hidden="true" className={cn("size-5", className)}>
      <path
        fill="currentColor"
        fillRule="evenodd"
        d="M320 256H704A256 256 0 0 1 704 768H320A256 256 0 0 1 320 256ZM192 512A128 128 0 1 0 448 512A128 128 0 1 0 192 512ZM640 448A64 64 0 0 0 640 576H768A64 64 0 0 0 768 448H640Z"
      />
    </svg>
  );
}
