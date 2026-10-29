/**
 * Unicode's White_Space, which a value's ends are trimmed of and a blank one
 * holds alone, as Go and Rust read them: `trim` differs.
 */
export const WHITE_SPACE = new Set('\t\n\v\f\r \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000');

/** `value` with Unicode's White_Space off its ends. */
export function trimEnds(value: string): string {
  let start = 0;
  while (start < value.length && WHITE_SPACE.has(value.charAt(start))) start++;
  let end = value.length;
  while (end > start && WHITE_SPACE.has(value.charAt(end - 1))) end--;
  return value.slice(start, end);
}

/** Whether `value` is empty or Unicode's White_Space alone. */
export function isBlank(value: string): boolean {
  return trimEnds(value) === '';
}
