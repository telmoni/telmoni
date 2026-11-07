// Where a console built on this one may take a new project at the plan's
// ceiling before this console asks auth for it: `null` here, so it is made at
// once, and the file a console that sells plans lays its own copy over (see
// `./ui`) answers where to send the person first — a payment page, say, after
// which it makes the project itself — or what refused it. `createProjectAction`
// asks it on the server, with the person's session, once the name has passed
// its checks, and only at the ceiling. `extras` are the fields the panel's
// notice (`components/extension/create-project-notice.tsx`) added, by name.

export interface CreateProjectInput {
  /// The organization it is made in.
  organizationId: string;
  /// The name, trimmed and within its length.
  name: string;
  /// The notice's own fields, by name.
  extras: Record<string, string>;
}

export type CreateProjectElsewhere =
  | {
      /// Where the person goes instead: a console path, or an absolute address
      /// off this console, which the panel leaves the page for.
      href: string;
    }
  | {
      /// The refusal the person reads, under the name.
      error: string;
    }
  | null;

export async function createProjectElsewhere(
  input: CreateProjectInput,
): Promise<CreateProjectElsewhere> {
  void input;
  return null;
}
