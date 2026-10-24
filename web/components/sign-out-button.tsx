import { Button } from "@/components/ui/button";

// "Sign out" wherever a plain link to `/auth/logout` once stood: a form post,
// as the account menu's is, since a GET with a live session only asks first.
// `contents` keeps the form out of its parent's layout.
export function SignOutButton({
  className,
  variant = "outline",
  size = "sm",
}: Pick<React.ComponentProps<typeof Button>, "className" | "variant" | "size">) {
  return (
    <form method="post" action="/auth/logout" className="contents">
      <Button type="submit" variant={variant} size={size} className={className}>
        Sign out
      </Button>
    </form>
  );
}
