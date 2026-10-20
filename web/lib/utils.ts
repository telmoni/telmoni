import { clsx, type ClassValue } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// ⚠ **tailwind-merge has to be told about the radius tokens that are OURS, or
// they lose every conflict silently.** It resolves `rounded-*` by matching
// known values — the t-shirt steps, `none`, `full` — and a name it does not
// recognise is not a radius to it, so it keeps BOTH classes and leaves the
// winner to CSS source order rather than to the order you wrote them in.
// Nothing about that failure is visible at the callsite: the class is spelled
// right, the token is real, and the element takes the other one's radius.
// Only the one token that sits between the `--radius-*` steps needs listing.
// The steps themselves, `none` and `full` are stock and already understood —
// listing one of those here would be a guard that passes whether or not this
// config exists.
const twMerge = extendTailwindMerge({
  extend: {
    theme: { radius: ["menu"] },
  },
});

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** How far a menu, popover, select or tooltip stands off the window's edges:
 *  the 14px every control keeps from an edge (`p-3.5`). Near one, it moves
 *  in, or scrolls inside that, rather than touch it. */
export const WINDOW_INSET = 14;
