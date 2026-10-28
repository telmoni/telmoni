/** A year: a cookie the browser leaves for the server outlives a session,
 *  and is written again whenever what it holds changes. */
export const YEAR_SECONDS = 365 * 24 * 60 * 60;

/** Leaves `name` for the server, on every path, for `maxAge` seconds; Secure
 *  where the page is. */
export function writeCookie(name: string, value: string, maxAge: number = YEAR_SECONDS): void {
  const secure = window.location.protocol === "https:" ? "; secure" : "";
  document.cookie = `${name}=${value}; path=/; max-age=${maxAge}; samesite=lax${secure}`;
}
