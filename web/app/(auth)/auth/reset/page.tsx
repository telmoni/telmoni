import Link from "next/link";

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

import { one } from "../_accounts/pending";
import { ResetForm } from "./_form";

export const metadata = { title: "Choose a new password" };
export const dynamic = "force-dynamic";

// The page the reset mail links to.
export default async function ResetPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const params = await searchParams;
  const userId = one(params.uid);
  const token = one(params.token);

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
