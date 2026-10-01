
const BIDI = /[\u061C\u200E\u200F\u202A-\u202E\u2066-\u2069]/gu;

const CONTROL =
  /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\u2028\u2029]/gu;

function spell(ch: string): string {
  const code = ch.codePointAt(0) ?? 0;
  return `<U+${code.toString(16).toUpperCase().padStart(4, "0")}>`;
}

export function escapeGuestText(text: string): string {
  return text.replace(BIDI, spell).replace(CONTROL, spell);
}
