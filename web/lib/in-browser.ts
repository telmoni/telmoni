import { useSyncExternalStore } from "react";

/** The subscribe of a snapshot nothing changes: what is read is fixed once
 *  the page is in the browser. */
export const subscribeNever = () => () => {};

/** Whether the page is in the browser yet: false as the server draws it and
 *  as it hydrates, true from the first client render on, so what only the
 *  browser knows is drawn there and its box stands in before. */
export function useInBrowser(): boolean {
  return useSyncExternalStore(
    subscribeNever,
    () => true,
    () => false,
  );
}
