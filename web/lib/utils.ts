import { clsx, type ClassValue } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// Custom tokens must resolve conflicts like Tailwind's stock radius and spacing scale.
const twMerge = extendTailwindMerge({
  extend: {
    theme: {
      radius: ["menu"],
      spacing: ["gutter", "page", "control", "console-header", "sidebar", "sidebar-collapsed", "sidebar-inner"],
    },
  },
});

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** A control's gutter in pixels: `--spacing-gutter` (`p-gutter`,
 *  `gap-gutter`) in `app/globals.css`. */
export const GUTTER_PX = 18;

/** The page inset and section gap: `--spacing-page` in `app/globals.css`.
 *  @alias */
export const PAGE_GUTTER_PX = GUTTER_PX;

/** From a header control's bottom to the outside of the header's rule. */
export const HEADER_MENU_OFFSET = GUTTER_PX + 1;

/** How far a menu, popover, select or tooltip stands off the window's edges:
 *  a gutter. Near one, it moves in, or scrolls inside that, rather than touch it.
 *  @alias */
export const WINDOW_INSET = GUTTER_PX;
