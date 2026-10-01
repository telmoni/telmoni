const LABELS: Record<string, string> = {
  password: "Email and password",
  passkey: "Passkey",
  magic_link: "Email link",
  sso: "Single sign-on",
  google: "Google",
  microsoft: "Microsoft",
  github: "GitHub",
  gitlab: "GitLab",
  bitbucket: "Bitbucket",
  apple: "Apple",
  linkedin: "LinkedIn",
  salesforce: "Salesforce",
  slack: "Slack",
  discord: "Discord",
  intuit: "Intuit",
  xero: "Xero",
  vercel: "Vercel",
  x: "X",
};

export function signInMethodLabel(
  method: string | null | undefined,
): string | null {
  if (!method) return null;
  if (Object.hasOwn(LABELS, method)) return LABELS[method]!;
  return method.charAt(0).toUpperCase() + method.slice(1);
}

// What the account settings page may offer about a password.
//
// - `reset` — an email-and-password account. The one posture with a control,
//   and the only one the Password section is rendered for at all.
// - `managed` — single sign-on. Their directory decides how they get in.
// - `provider` — any other connection (Google, Microsoft, a passkey, an email
//   link). There is no password on the account and we do not offer to add one.
// - `unknown` — the session does not say. Offers nothing, deliberately.
//
// ⚠ **The three non-`reset` postures render nothing.** External provider
// accounts do not require password management sections, as Profile's
// "Sign-in method" row already displays how they authenticate.
//
// So no caller reads the three apart today — every one of them asks
// `canResetPassword`, which is one bit. They are kept as vocabulary rather
// than collapsed to a boolean: each names a different REASON there is no
// password here, and those reasons are what the copy would have to say again
// the day this page offers any of them something. A boolean would have to
// rediscover them.
export type PasswordPosture = "reset" | "managed" | "provider" | "unknown";

// ⚠ **`provider` offers nothing.** Users who authenticate with an external
// provider do not manage a local password or email change here, ensuring an
// external account removal terminates access cleanly.
//
// `unknown` fails CLOSED for the same reason. A null method is a real state —
// an impersonated session, a migrated one, a cookie sealed before the field
// existed — and one of those people may well be a provider sign-up. Guessing
// wrong in the other direction offers a credential; guessing wrong this way
// costs one sign-out.
export function passwordPosture(
  method: string | null | undefined,
): PasswordPosture {
  if (!method) return "unknown";
  if (method === "password") return "reset";
  if (method === "sso") return "managed";
  return "provider";
}

// What the account settings page may offer about the address this account
// signs in with.
//
// ⚠ **A SECOND function, not a reading of `passwordPosture` above.** The two
// agree today by coincidence rather than by construction, and the likeliest
// divergence runs one way only: an email link is a plausible future `change`,
// because the address lives at our identity provider and its verified
// email-change flow would work on it. It must never become a password `reset`,
// which is the credential-minting trade `passwordPosture` exists to refuse.
// Asking one question through the other's answer means the day they diverge is
// a day somebody edits the password rule and silently changes what the email
// section says.
//
// - `change` — an email-and-password account. The one posture with a form,
//   and the only one the Email section is rendered for at all.
// - `managed` — single sign-on. The identity provider refuses to move a
//   directory-managed address at all, and the directory would assert the old
//   one back over anything we set.
// - `provider` — Google, Microsoft, a passkey, an email link. The provider
//   holds the address and re-asserts it at every sign-in, so a change made
//   here would last exactly until the next one.
// - `unknown` — the session does not say. Offers nothing, deliberately, for
//   `passwordPosture`'s reason: a null method may well be a provider sign-up,
//   and a change offered to one of those is a change that silently reverts.
//
// ⚠ Same rule as `passwordPosture` above, including its shape: the three
// non-`change` postures draw nothing at all rather than explaining their own
// absence, and no caller reads them apart — `canChangeEmail` is the one bit
// anybody asks for. They are the reasons, kept for the copy.
export type EmailPosture = "change" | "managed" | "provider" | "unknown";

export function emailPosture(
  method: string | null | undefined,
): EmailPosture {
  if (!method) return "unknown";
  if (method === "password") return "change";
  if (method === "sso") return "managed";
  return "provider";
}

// Whether this account may change the address it signs in with.
//
// One question, one answer: this is `emailPosture` read as a boolean, so the
// two can never disagree about who is eligible.
//
// It answers TWO questions that must not drift apart — whether the Email
// section is drawn, and whether the Server Action will act — and the second is
// the one that matters. The section being absent is presentation; the action's
// own check is the enforcement, because a Server Action is a public endpoint
// that a hidden control does nothing to protect.
export function canChangeEmail(method: string | null | undefined): boolean {
  return emailPosture(method) === "change";
}

// Whether this account has a password at our identity provider to reset.
//
// The mirror of `canChangeEmail`, and it exists so the settings page asks both
// questions the same way.
//
// ⚠ **Unlike `canChangeEmail`, this is presentation only — the Server Action
// does NOT re-check it, and the asymmetry is deliberate rather than an
// oversight.** `requestEmailChangeAction` has to gate, because the address it
// would write is one another organization can hold and a wrong one locks that
// organization out of `/me`. This lane writes nothing and mails nowhere but the
// address already on the row, so the worst an ineligible caller reaching the
// action directly can do is send themselves a link.
//
// What that link then does for an account with no password is the identity
// provider's answer and NOT VERIFIED here — it may refuse, or it may let them
// set one. If it turns out to mint a credential, this rule stops being cosmetic
// and the action needs the same gate `canChangeEmail` has.
export function canResetPassword(method: string | null | undefined): boolean {
  return passwordPosture(method) === "reset";
}
