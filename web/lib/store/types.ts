import type { FlagSet } from "@/lib/flags";
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

  renameProject: (id: string, name: string) => void;

  addProject: (project: Project) => void;

  addIncomingInvite: (invite: IncomingInvite) => void;

  removeIncomingInvite: (id: string) => void;

  /// Reconcile with a fresh server payload. Writes only the fields that moved
  /// since the last one, and nothing at all when none did.
  setSeed: (seed: StoreInitial) => void;
}

export type AppStore = IdentitySlice;

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
