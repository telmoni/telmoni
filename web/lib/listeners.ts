/** The subscribe and emit a module store hands `useSyncExternalStore`;
 *  `bind` runs while anyone listens: `first` as the first subscribes, `last`
 *  as the last leaves, so a `storage` listener lives only while it is read. */
export function listeners(bind?: { first: () => void; last: () => void }) {
  let set: ReadonlyArray<() => void> = [];
  return {
    subscribe(onChange: () => void): () => void {
      if (set.length === 0) bind?.first();
      set = [...set, onChange];
      return () => {
        set = set.filter((l) => l !== onChange);
        if (set.length === 0) bind?.last();
      };
    },
    emit(): void {
      for (const l of set) l();
    },
  };
}
