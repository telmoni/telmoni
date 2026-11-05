// Where a console built on this one keeps what its projects send, and who
// else processes it, as the organization's data sheet states it: none here,
// since a self-hosted deployment's stores are wherever its operator runs them,
// and the file such a console lays its own copy over (see `./ui`); the copy
// restates the interface.
export interface Residency {
  /// Where the stores and their backups are, in a sentence.
  where: string;
  /// The page that says more.
  href: string;
  /// Who else processes what the deployment keeps, as its own pages name them.
  subprocessors: readonly Subprocessor[];
}

/** One provider the deployment hands what it keeps to, as the data sheet lists it. */
export interface Subprocessor {
  provider: string;
  purpose: string;
  location: string;
}

export const RESIDENCY: Residency | null = null;
