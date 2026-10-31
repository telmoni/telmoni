// Where a console built on this one keeps what its projects send, as each
// project's Privacy page states it: none here, since a self-hosted
// deployment's stores are wherever its operator runs them, and the file such
// a console lays its own copy over (see `./ui`); the copy restates the
// interface.
export interface Residency {
  /// Where the stores and their backups are, in a sentence.
  where: string;
  /// The page that says more.
  href: string;
}

export const RESIDENCY: Residency | null = null;
