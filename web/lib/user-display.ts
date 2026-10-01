
export interface NameParts {
  firstName?: string | null;
  lastName?:  string | null;
  email:      string;
}

export function displayName({ firstName, lastName, email }: NameParts): string {
  const f = firstName?.trim() ?? "";
  const l = lastName?.trim()  ?? "";
  if (f && l) return `${f} ${l}`;
  if (f)      return f;
  return email.split("@")[0] || email;
}

