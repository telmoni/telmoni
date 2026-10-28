import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

import { sole } from "@/lib/list-query";
import { ConfirmEmail } from "./_confirm";

export const metadata = { title: "Confirm your email address" };

// The page the verification mail links to. It spends nothing on load: a mail
// scanner that opens every link would otherwise spend it before the person
// did, and the button is what does.
export default async function VerifyPage({
  searchParams,
}: PageProps<"/auth/verify">) {
  const params = await searchParams;
  const userId = sole(params.uid);
  const token = sole(params.token);

  return (
    <Card className="gap-3">
      <CardHeader>
        <CardTitle>Confirm your email address</CardTitle>
        <CardDescription>One press, and the account is yours to sign in to.</CardDescription>
      </CardHeader>
      <CardContent>
        {userId && token ? (
          <ConfirmEmail userId={userId} token={token} />
        ) : (
          <p className="text-sm text-muted-foreground">
            This link is incomplete. Open it from the email again, or sign in with your password
            to have another sent.
          </p>
        )}
      </CardContent>
    </Card>
  );
}
