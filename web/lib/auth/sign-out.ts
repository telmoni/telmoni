// Signing out is a post, never a navigation: `GET /auth/logout` asks a live
// session first, since a link is all it takes to get there. The account menu's
// form, built here for every other button and timer that signs out.
export function signOut(): void {
  const form = document.createElement("form");
  form.method = "POST";
  form.action = "/auth/logout";
  document.body.appendChild(form);
  form.submit();
}
