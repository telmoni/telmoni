// The reader's clock as the trace list's histogram needs it: the bars start
// where their hours and days start, not UTC's. The server renders the page
// and cannot ask the browser, so the browser leaves its offset in a cookie
// (`components/traces/utc-offset.tsx`) and the page reads it on the next
// render. An offset is coarse — half the world shares each — and is all the
// cookie holds.

/** The cookie the browser leaves its offset in. */
export const UTC_OFFSET_COOKIE = "telmoni-utc-offset";

/** The furthest west and east a clock is set from UTC, in minutes: UTC−12
 *  and UTC+14, the bounds the server takes. */
export const UTC_OFFSET_WEST = -720;
export const UTC_OFFSET_EAST = 840;

/** The cookie's offset in whole minutes east of UTC, or UTC itself where
 *  there is none or it is not one. */
export function parseUtcOffset(value: string | undefined): number {
  if (value === undefined || !/^-?\d{1,3}$/.test(value)) return 0;
  const minutes = Number(value);
  return minutes >= UTC_OFFSET_WEST && minutes <= UTC_OFFSET_EAST ? minutes : 0;
}

/** This browser's offset now, in whole minutes east of UTC; never `-0`. */
export function browserUtcOffset(date: Date = new Date()): number {
  return -date.getTimezoneOffset() || 0;
}
