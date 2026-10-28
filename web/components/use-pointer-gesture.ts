"use client";

import { useRef, useState, type PointerEvent } from "react";

/** The handlers the pressed element takes, which follow a press to its end. */
export type GestureHandlers<E extends HTMLElement> = {
  onPointerMove: (e: PointerEvent<E>) => void;
  onPointerUp: (e: PointerEvent<E>) => void;
  onPointerCancel: (e: PointerEvent<E>) => void;
  onLostPointerCapture: (e: PointerEvent<E>) => void;
};

/** A gesture one pointer holds from a press to its release — a drag of a
 *  panel's edge or of the agent's window — followed through the pointer's
 *  capture, so only that pointer's moves and release are its own; a cancel
 *  or a lost capture ends it as a release does. */
export function usePointerGesture<G, E extends HTMLElement = HTMLElement>({
  move,
  end,
}: {
  /** What the gesture holds after a move, from what it held before. */
  move: (held: G, e: PointerEvent<E>) => G;
  /** Told once, with what the gesture held as it ended. */
  end: (held: G) => void;
}): {
  /** What the pointer holds, `null` between gestures. */
  held: G | null;
  /** A press that starts a gesture holding `held`; a secondary button's or
   *  a second pointer's starts none. */
  press: (e: PointerEvent<E>, held: G) => void;
  handlers: GestureHandlers<E>;
} {
  const [held, setHeld] = useState<G | null>(null);
  const live = useRef<{ pointerId: number; held: G } | null>(null);
  const press = (e: PointerEvent<E>, next: G) => {
    if (e.button !== 0 || !e.isPrimary) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    live.current = { pointerId: e.pointerId, held: next };
    setHeld(next);
  };
  const onPointerMove = (e: PointerEvent<E>) => {
    const g = live.current;
    if (!g || g.pointerId !== e.pointerId) return;
    g.held = move(g.held, e);
    setHeld(g.held);
  };
  const release = (e: PointerEvent<E>) => {
    const g = live.current;
    if (!g || g.pointerId !== e.pointerId) return;
    live.current = null;
    setHeld(null);
    end(g.held);
  };
  return {
    held,
    press,
    handlers: {
      onPointerMove,
      onPointerUp: release,
      onPointerCancel: release,
      onLostPointerCapture: release,
    },
  };
}
