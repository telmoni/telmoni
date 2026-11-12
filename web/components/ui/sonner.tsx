"use client";

import {
  CircleCheckIcon,
  InfoIcon,
  Loader2Icon,
  OctagonXIcon,
  TriangleAlertIcon,
} from "lucide-react";
import { useTheme } from "next-themes";
import { Toaster as Sonner, type ToasterProps } from "sonner";

const Toaster = ({ ...props }: ToasterProps) => {
  const { theme = "system" } = useTheme();

  return (
    <Sonner
      theme={theme as ToasterProps["theme"]}
      className="toaster group"
      position="bottom-right"
      closeButton
      icons={{
        success: <CircleCheckIcon className="size-icon" />,
        info: <InfoIcon className="size-icon" />,
        warning: <TriangleAlertIcon className="size-icon" />,
        error: <OctagonXIcon className="size-icon" />,
        loading: <Loader2Icon className="size-icon animate-spin" />,
      }}
      toastOptions={{
        // Sonner sets 13px on the toast itself, so the container's size never reaches it.
        className: "!shadow-2xl !text-sm",
      }}
      style={
        {
          fontFamily: "var(--font-sans)",
          fontSize: "var(--text-sm)",
          "--normal-bg": "var(--accent)",
          "--normal-text": "var(--accent-foreground)",
          "--normal-border": "transparent",
          // The toast's radius; its buttons take the same one in
          // `globals.css`, where the sharp theme squares all of it. Never a
          // `!rounded-*` class here — it would outrank that rule.
          "--border-radius": "var(--radius)",
        } as React.CSSProperties
      }
      {...props}
    />
  );
};

export { Toaster };
