import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { getServerSession } from "@/lib/server/session";
import { lookUpInvite } from "@/lib/server/data";
import { PRODUCT_NAME } from "@/lib/site";

import { AcceptInvite } from "./_accept";

export const metadata = { title: "Invitation" };

export default async function InvitePage({
  params,
}: {
  params: Promise<{ token: string }>;
}) {
  const { token } = await params;
  const [invite, session] = await Promise.all([
    lookUpInvite(token),
    getServerSession(),
  ]);

  if (!invite) {
    return (
      <Shell>
        <h1 className="text-base font-medium">This invitation has expired</h1>
        <p className="text-sm text-muted-foreground">
          It may have been used already, or withdrawn, or simply run out of
          time. Ask whoever sent it to invite you again.
        </p>
      </Shell>
    );
  }

  // The two ladders spell their roles the same, so the level is auth's word
  // (`scope`), never inferred from the role.
  const isOrganizationScope = invite.scope === "organization";
  const roleSentence = isOrganizationScope
    ? invite.role === "admin"
      ? "You would be an admin of the organization: you could create projects, manage members and settings, and work as an admin on every project."
      : "You would be a member of the organization, with access to the projects you are given."
    : invite.role === "admin"
      ? "You would be an admin on the project: you could see its members, keys, connectors and audit log."
      : "You would be a member of the project, with read-only access.";

  const offer = (
    <>
      <h1 className="text-base font-medium">
        {invite.inviter} invited you to{" "}
        {isOrganizationScope
          ? `${invite.organization} on ${PRODUCT_NAME}`
          : `a project in ${invite.organization} on ${PRODUCT_NAME}`}
      </h1>
      <p className="text-sm text-muted-foreground">
        {roleSentence} Nothing of yours is shared with them, and you can leave
        at any time.
      </p>
    </>
  );

  if (!session) {
    const back = `/invite/${encodeURIComponent(token)}`;
    return (
      <Shell>
        {offer}
        <p className="text-sm">
          Sign in as <span className="font-medium">{invite.email}</span> to
          accept it. That address is the one it was sent to, and the only one
          that can take it.
        </p>
        <div className="flex flex-wrap gap-3">
          <Button asChild>
            <a href={`/auth/signup?returnTo=${encodeURIComponent(back)}`}>
              Create an account
            </a>
          </Button>
          <Button variant="outline" asChild>
            <a href={`/auth/login?returnTo=${encodeURIComponent(back)}`}>
              Sign in
            </a>
          </Button>
        </div>
      </Shell>
    );
  }

  if (session.email?.toLowerCase() !== invite.email.toLowerCase()) {
    return (
      <Shell>
        {offer}
        <p className="text-sm">
          You are signed in as{" "}
          <span className="font-medium">{session.email}</span>, and this
          invitation was sent to{" "}
          <span className="font-medium">{invite.email}</span>. Sign in with that
          address to accept it.
        </p>
        <div>
          <Button variant="outline" asChild>
            {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
            <a href="/auth/logout">Sign out</a>
          </Button>
        </div>
      </Shell>
    );
  }

  return (
    <Shell>
      {offer}
      <AcceptInvite token={token} />
    </Shell>
  );
}

function Shell({ children }: { children: React.ReactNode }) {
  return (
    <div className="theme-paper flex min-h-svh flex-col items-center justify-center bg-background p-8">
      <Card className="grid w-full max-w-md gap-3">{children}</Card>
    </div>
  );
}
