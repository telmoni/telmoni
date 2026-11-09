"use client";

import { useEffect, useState, type RefObject } from "react";

/** Which way the wrapper's first child, a scroller, still runs past its edges
 *  along `axis`: `"left right"`, `"left"`, `"right"`, or `"top bottom"`,
 *  `"top"`, `"bottom"`, or `""`, for `[data-overflow]` in `globals.css` to
 *  fade that edge. Measured again as it scrolls and as it or what it holds
 *  changes size. */
export function useOverflow(wrapper: RefObject<HTMLElement | null>, axis: "x" | "y" = "x"): string {
  const [overflow, setOverflow] = useState("");
  useEffect(() => {
    const scroller = wrapper.current?.firstElementChild;
    if (!(scroller instanceof HTMLElement)) return;
    const [start, end] = axis === "x" ? ["left", "right"] : ["top", "bottom"];
    const measure = () => {
      const [at, seen, whole] =
        axis === "x"
          ? [scroller.scrollLeft, scroller.clientWidth, scroller.scrollWidth]
          : [scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight];
      const before = at > 0;
      const after = at + seen < whole - 1;
      setOverflow(before && after ? `${start} ${end}` : before ? start : after ? end : "");
    };
    measure();
    scroller.addEventListener("scroll", measure, { passive: true });
    const observer = new ResizeObserver(measure);
    observer.observe(scroller);
    for (const child of scroller.children) observer.observe(child);
    return () => {
      scroller.removeEventListener("scroll", measure);
      observer.disconnect();
    };
  }, [wrapper, axis]);
  return overflow;
}
