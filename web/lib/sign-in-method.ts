// How the Profile card names the way a sign-in was proved. The password form
// is the one method the core's auth records: an OpenID Connect sign-in records
// none (`crates/auth/src/oidc/mod.rs`), so its row is left out. Any other name
// is a deployment's own provider's, shown with its first letter capitalised.
export function signInMethodLabel(
  method: string | null | undefined,
): string | null {
  if (!method) return null;
  if (method === "password") return "Email and password";
  return method.charAt(0).toUpperCase() + method.slice(1);
}

// What the account settings page may offer about a password.
//
// - `reset` — an email-and-password account. The one posture with a control,
//   and the only one the Password section is rendered for at all.
// - `provider` — a method a deployment's own sign-in provider reports. There
//   is no password on the account and we do not offer to add one.
// - `unknown` — the session does not say, as an OpenID Connect sign-in does
//   not. Offers nothing, deliberately.
//
// ⚠ **The two non-`reset` postures render nothing.** An account that signs in
// elsewhere manages its password there.
//
// So no caller reads the two apart today — every one of them asks
// `canResetPassword`, which is one bit. They are kept as vocabulary rather
// than collapsed to a boolean: each names a different REASON there is no
// password here, and those reasons are what the copy would have to say again
// the day this page offers either of them something. A boolean would have to
// rediscover them.
export type PasswordPosture = "reset" | "provider" | "unknown";

// ⚠ **`provider` offers nothing.** Users who authenticate with an external
// provider do not manage a local password or email change here, ensuring an
// external account removal terminates access cleanly.
//
// `unknown` fails CLOSED for the same reason. A null method is what an OpenID
// Connect sign-in records, or a session sealed before the field existed, and
// the person behind either may have no password here at all. Guessing wrong
// in the other direction offers a credential; guessing wrong this way costs
// one sign-out.
export function passwordPosture(
  method: string | null | undefined,
): PasswordPosture {
  if (!method) return "unknown";
  if (method === "password") return "reset";
  return "provider";
}

// What the account settings page may offer about the address this account
// signs in with.
//
// ⚠ **A SECOND function, not a reading of `passwordPosture` above.** The two
// agree today by coincidence rather than by construction: moving an address
// and resetting a password are different trades, and only the second mints a
// credential, which `passwordPosture` exists to refuse. Asking one question
// through the other's answer means the day they diverge is a day somebody
// edits the password rule and silently changes what the email section says.
//
// - `change` — an email-and-password account. The one posture with a form,
//   and the only one the Email section is rendered for at all.
// - `provider` — a method a deployment's own sign-in provider reports. The
//   provider holds the address and re-asserts it at every sign-in, so a change
//   made here would last exactly until the next one.
// - `unknown` — the session does not say. Offers nothing, deliberately, for
//   `passwordPosture`'s reason: a null method may well be a provider sign-up,
//   and a change offered to one of those is a change that silently reverts.
//
// ⚠ Same rule as `passwordPosture` above, including its shape: the two
// non-`change` postures draw nothing at all rather than explaining their own
// absence, and no caller reads them apart — `canChangeEmail` is the one bit
// anybody asks for. They are the reasons, kept for the copy.
export type EmailPosture = "change" | "provider" | "unknown";

export function emailPosture(
  method: string | null | undefined,
): EmailPosture {
  if (!method) return "unknown";
  if (method === "password") return "change";
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
