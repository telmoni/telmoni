// What a person typed into a number field, kept to what a number of its kind
// is written with, as a code field keeps a code's digits (`confirmation-code.ts`):
// a letter typed or pasted never reaches the draft. Grouping commas stay,
// which the forms' own reading allows (`lib/alerts.ts`, `parsed`).

/** A whole number's digits. */
export function wholeTyped(raw: string): string {
  return raw.replace(/\D/g, "");
}

/** A decimal's digits, its one point and grouping commas, and a leading minus
 *  where the field takes one below 0. */
export function decimalTyped(raw: string, signed = false): string {
  const kept = raw.replace(signed ? /[^\d.,-]/g : /[^\d.,]/g, "");
  const minus = signed && kept.startsWith("-") ? "-" : "";
  const [whole = "", ...fraction] = kept.replace(/-/g, "").split(".");
  return minus + (fraction.length > 0 ? `${whole}.${fraction.join("")}` : whole);
}
