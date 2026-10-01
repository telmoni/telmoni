import { clsx, type ClassValue } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// ⚠ **tailwind-merge has to be told about the radius tokens that are OURS, or
// they lose every conflict silently.** It resolves `rounded-*` by matching
// known values — the t-shirt steps, `none`, `full` — and a name it does not
// recognise is not a radius to it, so it keeps BOTH classes and leaves the
// winner to CSS source order rather than to the order you wrote them in.
// Nothing about that failure is visible at the callsite: the class is spelled
// right, the token is real, and the element takes the other one's radius.
// Only the two surfaces that sit between the `--radius-*` steps need listing.
// The steps themselves, `none` and `full` are stock and already understood —
// listing one of those here would be a guard that passes whether or not this
// config exists.
const twMerge = extendTailwindMerge({
  extend: {
    theme: { radius: ["console-surface", "menu"] },
  },
});

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
