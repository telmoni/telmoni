import { isOrganizationSlug, isSlug, SLUG_MAX_LENGTH } from "@/lib/slug";

/// The names a person types, held to auth's rules as the forms check them
/// first: counted in characters, as auth counts them, never UTF-16 units.
export const MAX_PROJECT_NAME = 100;
export const MAX_ORGANIZATION_NAME = 80;
export const MAX_API_KEY_NAME = 100;
export const MAX_API_KEY_DESCRIPTION = 500;

/// The longest email address auth takes.
export const MAX_EMAIL = 254;

/// An input's `maxLength` counts UTF-16 units, two to a character at most:
/// a loose bound on typing, never the rule.
export const typedBound = (max: number) => max * 2;

export type Checked = { ok: true; value: string } | { ok: false; error: string };

const characters = (text: string) => [...text].length;

export function checkProjectName(name: string): Checked {
  const value = name.trim();
  if (!value) return { ok: false, error: "Project name cannot be empty." };
  if (characters(value) > MAX_PROJECT_NAME) {
    return { ok: false, error: `Project name must be ${MAX_PROJECT_NAME} characters or fewer.` };
  }
  return { ok: true, value };
}

export function checkApiKeyName(name: string): Checked {
  const value = name.trim();
  if (!value || characters(value) > MAX_API_KEY_NAME) {
    return { ok: false, error: `Name the key: 1 to ${MAX_API_KEY_NAME} characters.` };
  }
  return { ok: true, value };
}

export function checkApiKeyDescription(description: string): Checked {
  const value = description.trim();
  if (characters(value) > MAX_API_KEY_DESCRIPTION) {
    return {
      ok: false,
      error: `Keep the description to ${MAX_API_KEY_DESCRIPTION} characters or fewer.`,
    };
  }
  return { ok: true, value };
}

export function checkOrganizationName(name: string): Checked {
  const value = name.trim();
  if (!value) return { ok: false, error: "Give your organization a name." };
  if (characters(value) > MAX_ORGANIZATION_NAME) {
    return { ok: false, error: `Keep it to ${MAX_ORGANIZATION_NAME} characters or fewer.` };
  }
  return { ok: true, value };
}

/// An organization's URL, by its shape alone: whether it is free is auth's
/// to say.
export function checkOrganizationUrl(slug: string): Checked {
  const value = slug.trim();
  if (!isSlug(value)) {
    return {
      ok: false,
      error: `A URL is lowercase letters and digits, in words joined by single hyphens, at most ${SLUG_MAX_LENGTH} characters.`,
    };
  }
  if (!isOrganizationSlug(value)) {
    return { ok: false, error: `${value} is a word the console's own pages use — choose another.` };
  }
  return { ok: true, value };
}
