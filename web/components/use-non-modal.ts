"use client";

import { useEffect, useEffectEvent, useLayoutEffect, useRef, type RefObject } from "react";

/** A panel that stands beside the page without shutting it off, the side
 *  panel and the agent's window alike: it takes the focus as it opens, hands
 *  it back as it shuts if the focus is still inside, and shuts on Escape
 *  pressed inside it. Only inside: Escape on the page is the page's. */
export function useNonModal(
  panel: RefObject<HTMLElement | null>,
  {
    onClose,
    initialFocus,
    returnFocus,
  }: {
    onClose: () => void;
    /** What takes the focus as it opens; the panel itself if none, or if
     *  that will not take it, as a field disabled for now will not. */
    initialFocus?: RefObject<HTMLElement | null>;
    /** Where the focus goes as it shuts with the focus inside; the element
     *  that had it before it opened if this answers none. */
    returnFocus?: () => HTMLElement | null | undefined;
  },
): void {
  // Read as the panel shuts, in the layout effect's cleanup, which an Effect
  // Event may not serve.
  const back = useRef(returnFocus);
  useEffect(() => {
    back.current = returnFocus;
  });
  const shut = useEffectEvent(() => onClose());

  // A layout effect, so the panel is still in the document when its cleanup
  // asks where the focus is: shut by a click on the page beside it, the
  // focus is that click's.
  useLayoutEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const element = panel.current;
    const first = initialFocus?.current ?? element;
    first?.focus({ preventScroll: true });
    if (document.activeElement !== first) element?.focus({ preventScroll: true });
    return () => {
      if (!element?.contains(document.activeElement)) return;
      (back.current?.() ?? before)?.focus({ preventScroll: true });
    };
  }, [panel, initialFocus]);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      if (!panel.current?.contains(document.activeElement)) return;
      e.preventDefault();
      shut();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [panel]);
}
