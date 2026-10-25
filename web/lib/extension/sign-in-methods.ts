// The sign-in methods a console built on this one records beyond the core's
// password form (`auth_method`, `crates/auth/src/provider.rs`): how the Profile
// card names each, and which may change their address in the console — one
// whose provider moves the address on request, such as a code mailed to it,
// rather than re-asserting its own at every sign-in. Empty here, since an
// OpenID Connect sign-in records no method, and the file such a console lays
// its own copy over (see `./ui`); the copy restates the interface.
export interface SignInMethod {
  /// The Profile card's name for it.
  label: string;
  /// Whether the Email section may move the address, through the provider.
  emailChange: boolean;
}

export const SIGN_IN_METHODS: ReadonlyMap<string, SignInMethod> = new Map();
