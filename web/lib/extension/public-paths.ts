// Path prefixes a console built on this one serves without a session, such as
// a public pricing page: empty here, and the file such a console lays its own
// copy over (see `./ui`). Each matches itself and its subtree, as the core's
// own prefixes in `lib/proxy/public-paths.ts` do.
export const EXTRA_PUBLIC_PREFIXES: readonly string[] = [];
