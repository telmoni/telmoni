import Link from "next/link";
import { redirect } from "next/navigation";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { getServerSession } from "@/lib/server/session";

export const metadata = { title: "Sign out" };

// Where `GET /auth/logout` sends a live session: a link is all it takes to
// arrive there, so the sign-out itself is the post this page's button makes.
export default async function ConfirmSignOutPage() {
  if ((await getServerSession()) === null) redirect("/");

  return (
    <Card className="gap-3">
      <CardHeader>
        <CardTitle>Sign out?</CardTitle>
        <CardDescription>
          A link brought you to the sign-out page. Signing out ends this session
          and takes you to the home page; nothing happens until you choose.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex gap-2">
        <form method="post" action="/auth/logout">
          <Button type="submit">Sign out</Button>
        </form>
        <Button asChild variant="outline">
          <Link href="/console">Stay signed in</Link>
        </Button>
      </CardContent>
    </Card>
  );
}
