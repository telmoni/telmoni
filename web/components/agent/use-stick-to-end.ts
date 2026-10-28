import { useEffect, useRef } from "react";

// Within this of the end, a person is reading the end, and a reply that grows
// keeps it in view; further up, they are reading back, and are left there.
const PINNED_PX = 48;

/** Keeps the end of a column in view as it grows — a streaming reply, a
 *  conversation opened, the window made larger — while the person is reading
 *  the end of it; `active` while the column is on show. */
export function useStickToEnd(active: boolean) {
  const bodyRef = useRef<HTMLDivElement>(null);
  const columnRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  useEffect(() => {
    if (!active) return;
    const body = bodyRef.current;
    const column = columnRef.current;
    if (!body || !column) return;
    const stick = () => {
      if (pinned.current) body.scrollTop = body.scrollHeight;
    };
    stick();
    const observer = new ResizeObserver(stick);
    observer.observe(column);
    return () => observer.disconnect();
  }, [active]);
  return {
    bodyRef,
    columnRef,
    /** The body scrolled: it stays pinned to the end while near it. */
    onScroll: () => {
      const body = bodyRef.current;
      if (body) pinned.current = body.scrollHeight - body.scrollTop - body.clientHeight < PINNED_PX;
    },
    /** Back to the end: a question sent, a conversation opened. */
    pin: () => {
      pinned.current = true;
    },
  };
}
