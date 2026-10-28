// The console's one answer to "does this string stay on this origin", for
// every place a path is followed — a return after sign-in, a visit kept, a
// link the agent writes, the trail — so no caller copies a weaker rule.

const PATH_BASE = "http://console.invalid";

/** `raw` resolved as a path on this console, or `null` where it would leave
 *  it. Only a path is one, with nothing a browser would read as more: no
 *  control character or backslash, no other origin, and no `//` left where
 *  `..` climbed to the root, which a caller re-resolving it would read as
 *  another host. */
export function sameOriginUrl(raw: string, base: string = PATH_BASE): URL | null {
  if (!raw.startsWith("/") || /[\u0000-\u001f\u007f\\]/.test(raw)) return null;
  let url: URL;
  try {
    url = new URL(raw, base);
  } catch {
    return null;
  }
  return url.origin === new URL(base).origin && !url.pathname.startsWith("//") ? url : null;
}
