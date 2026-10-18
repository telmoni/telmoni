// Where a console built on this one may take the founding of an organization
// before this console asks auth for it: `null` here, so it is founded at once,
// and the file a console that sells plans lays its own copy over (see `./ui`)
// answers where to send the person first — a payment page, say, after which
// it founds the organization itself — or what refused it, under which of the
// panel's fields. `createOrganizationAction` asks it on the server, with the
// person's session, once the name and URL have passed every founding's
// checks. `extras` are the fields the panel's notice
// (`components/extension/create-organization-notice.tsx`) added, by name.

export interface CreateOrganizationInput {
  /// The name, trimmed and within its length.
  name: string;
  /// The URL asked for, a slug or empty.
  slug: string;
  /// The notice's own fields, by name.
  extras: Record<string, string>;
}

export type CreateOrganizationElsewhere =
  | {
      /// Where the person goes instead: a console path, or an absolute address
      /// off this console, which the panel leaves the page for.
      href: string;
    }
  | {
      /// The refusal the person reads, under one of the panel's fields or
      /// below them both.
      error: string;
      field?: "name" | "slug";
    }
  | null;

export async function createOrganizationElsewhere(
  input: CreateOrganizationInput,
): Promise<CreateOrganizationElsewhere> {
  void input;
  return null;
}
