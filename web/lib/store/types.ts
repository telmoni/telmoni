import type { FlagSet } from "@/lib/flags";
import type { AuditExport } from "@/lib/types/audit-export";
import type { AuthUser } from "@/lib/types/domain";
import type { Role } from "@/lib/types/enums";
import type { Project, ProjectEverywhere } from "@/lib/server/entities/projects";
import type {
  IncomingInvite,
  OrganizationEntry,
  ProjectOffer,
} from "@/lib/server/entities/organization";

/// Everything the server tells the console about who is asking, resolved: no
/// optional fields and no fallbacks left to apply. `StoreInitial` is what the
/// layout hands over, `Seed` is what that means once `normalizeSeed` has read
/// it, and the two differ on purpose — the resolution rules live in one place
/// rather than at each callsite.
export interface Seed {
  user: AuthUser | null;
  /// Every organization the person is in, with their role in each. Theirs are
  /// the entries where the role is `owner`; there is no separate "own"
  /// organization.
  organizations: readonly OrganizationEntry[];
  incomingInvites: readonly IncomingInvite[];
  /// The projects offered to the person and still open, which the bell
  /// counts beside the invitations and the organization offers.
  projectOffers: readonly ProjectOffer[];
  activeOrganizationId: string | null;
  flags: FlagSet;
  roles: Readonly<Record<string, Role>>;
  projects: readonly Project[];
  projectsElsewhere: readonly ProjectEverywhere[];
}

export interface IdentitySlice extends Seed {
  /// ⚠ **Bookkeeping, not view state.** The last seed the server handed over,
  /// held so `setSeed` can answer "is this the payload I already applied?"
  /// against what the SERVER last said rather than against the live state —
  /// which a local edit has already moved. No hook exposes it.
  serverSeed: Seed;

  /// A rename moves the slug with the name; both are the server's answer.
  renameProject: (id: string, name: string, slug: string) => void;

  addProject: (project: Project) => void;

  addIncomingInvite: (invite: IncomingInvite) => void;

  removeIncomingInvite: (id: string) => void;

  /// Reconcile with a fresh server payload. Writes only the fields that moved
  /// since the last one, and nothing at all when none did.
  setSeed: (seed: StoreInitial) => void;
}

/// The person's exports of one organization's audit log. Never in the seed:
/// the bell asks for them once the console is on screen, so no page render
/// waits on them.
export interface AuditExportsSlice {
  /// Their latest exports in each organization the bell has asked about,
  /// newest first. Kept per organization, so one still building when the
  /// person moves to another is announced when they come back.
  auditExports: Readonly<Record<string, readonly AuditExport[]>>;

  /// How many exports were started from this tab: the mark a read of the
  /// list is held to, since a list read before the latest start lacks it.
  auditExportsStarted: number;

  /// Lay a list auth answered over the store's, unless an export was started
  /// here after the read left (`startedBefore` is the count it left with):
  /// that list would drop it, and with it the polling that announces it.
  /// Answers what the store held for the organization before, or `null` when
  /// it kept its own.
  replaceAuditExports: (
    organizationId: string,
    exports: readonly AuditExport[],
    startedBefore: number,
  ) => readonly AuditExport[] | null;

  /// One just started, ahead of the list's next read.
  addAuditExport: (organizationId: string, entry: AuditExport) => void;

  /// Taken: the bell stops offering it, as auth will say on the next read.
  markAuditExportDownloaded: (id: string) => void;

  /// No longer kept — past its week, or deleted to make room for a newer
  /// one — as a download just learned: gone from every list now, as auth
  /// will say on the next read, rather than offered again until then.
  forgetAuditExport: (id: string) => void;
}

export type AppStore = IdentitySlice & AuditExportsSlice;

export interface StoreInitial {
  user: AuthUser | null;
  organizations?: readonly OrganizationEntry[];
  incomingInvites?: readonly IncomingInvite[];
  projectOffers?: readonly ProjectOffer[];
  activeOrganizationId?: string | null;
  flags: FlagSet;
  roles: Readonly<Record<string, Role>>;
  projects: readonly Project[];
  projectsElsewhere?: readonly ProjectEverywhere[];
}
