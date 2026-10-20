"use client";

/** Moves the focus without following the fragment, whose history entry has no
 *  state: Next's router passes over one, which would leave a peek open on Back. */
export function SkipLink() {
  return (
    <a
      href="#console-main"
      onClick={(event) => {
        // One asking for another tab or window is the link's own.
        if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
        const main = document.getElementById("console-main");
        // A panel over the page whole stands in its place, the page out of reach.
        const content = main?.inert
          ? [...document.querySelectorAll<HTMLElement>('[data-slot="side-panel"]')].at(-1)
          : main;
        if (!content) return;
        event.preventDefault();
        content.focus();
      }}
      className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-4 focus:z-[60] focus:rounded-md focus:bg-card focus:px-3 focus:py-2 focus:text-sm focus:text-foreground focus:border focus:border-border"
    >
      Skip to content
    </a>
  );
}
