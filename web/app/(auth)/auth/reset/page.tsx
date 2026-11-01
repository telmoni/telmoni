import Link from "next/link";
import { notFound } from "next/navigation";

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { getSignInConfig } from "@/lib/auth/oidc";

import { sole } from "@/lib/list-query";
import { ResetForm } from "./_form";

export const metadata = { title: "Choose a new password" };

// The page the reset mail links to.
export default async function ResetPage({
  searchParams,
}: PageProps<"/auth/reset">) {
  const params = await searchParams;
  const userId = sole(params.uid);
  const token = sole(params.token);
  // As the page that mails the link: none while the password form is off.
  const config = await getSignInConfig();
  if (config && !config.passwordSignIn) notFound();

  return (
    <Card className="gap-3">
      <CardHeader>
        <CardTitle>Choose a new password</CardTitle>
        <CardDescription>
          Every signed-in session ends when you do, this one included.
        </CardDescription>
      </CardHeader>
      <CardContent>
        {userId && token ? (
          <ResetForm userId={userId} token={token} />
        ) : (
          <p className="text-sm text-muted-foreground">
            This link is incomplete. Open it from the email again, or{" "}
            <Link href="/auth/forgot" className="underline underline-offset-4">
              ask for a new one
            </Link>
            .
          </p>
        )}
      </CardContent>
    </Card>
  );
}
