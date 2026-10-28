// The one place an id's shape is checked before it becomes a header or a path
// segment: no separator a path could misread. What an id is beyond that is the
// server's to say.
const SAFE = /^[A-Za-z0-9_-]{1,64}$/;

/** A Server Action takes whatever was posted, whatever its parameter's type
 *  says, and `RegExp.test` would read `["project_1"]` as its string. Not a
 *  type guard: a caller's `string` refused here would narrow to `never`. */
export function isProjectId(value: unknown): boolean {
  return typeof value === "string" && value.startsWith("project_") && SAFE.test(value);
}

export function isOrganizationId(value: unknown): boolean {
  return typeof value === "string" && value.startsWith("org_") && SAFE.test(value);
}
