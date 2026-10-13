// Where the console goes once the person has created an organization: `null`
// here, its Overview, and the file a console built on this one lays its own
// copy over (see `./ui`) — one that sells plans sends an organization that
// starts on none to its billing. `createOrganizationAction` asks it on the
// server, with the person's session, once auth has answered with the new
// organization.
export async function createdOrganizationHref(created: {
  id: string;
  slug: string;
}): Promise<string | null> {
  void created;
  return null;
}
