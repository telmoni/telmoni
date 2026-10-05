// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const refresh = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", () => ({ useRouter: () => ({ refresh }) }));

const toastError = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({
  toast: { error: toastError, success: toastSuccess },
}));

const setDefaultOrganizationAction = vi.hoisted(() =>
  vi.fn<(organizationId: string) => Promise<{ error: string | null }>>(async () => ({
    error: null,
  })),
);
vi.mock("./actions", () => ({ setDefaultOrganizationAction }));

// Radix's listbox opens on pointer events jsdom does not dispatch; a native
// select stands in, with the same value in and the same value out.
vi.mock("@/components/ui/select", () => ({
  Select: ({
    value,
    onValueChange,
    disabled,
    children,
  }: {
    value: string;
    onValueChange: (v: string) => void;
    disabled?: boolean;
    children: React.ReactNode;
  }) => (
    <select
      aria-label="Default organization"
      value={value}
      disabled={disabled}
      onChange={(e) => onValueChange(e.target.value)}
    >
      {children}
    </select>
  ),
  SelectTrigger: () => null,
  SelectValue: () => null,
  SelectContent: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  SelectItem: ({ value, children }: { value: string; children: React.ReactNode }) => (
    <option value={value}>{children}</option>
  ),
}));

import { DefaultOrganization } from "./_default-organization";

const ORGANIZATIONS = [
  { organizationId: "org_1", label: "Acme" },
  { organizationId: "org_2", label: "Beta" },
];

beforeEach(() => {
  vi.clearAllMocks();
  setDefaultOrganizationAction.mockResolvedValue({ error: null });
});

function choose(organizationId: string) {
  fireEvent.change(screen.getByRole("combobox", { name: "Default organization" }), {
    target: { value: organizationId },
  });
}

describe("DefaultOrganization", () => {
  it("starts from the default auth answers, and saves nothing until another is chosen", () => {
    render(<DefaultOrganization organizations={ORGANIZATIONS} current="org_1" />);
    expect(screen.getByRole("combobox", { name: "Default organization" })).toHaveValue("org_1");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("saves the organization chosen, then reads the page again", async () => {
    render(<DefaultOrganization organizations={ORGANIZATIONS} current="org_1" />);
    choose("org_2");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });

    expect(setDefaultOrganizationAction).toHaveBeenCalledWith("org_2");
    expect(toastSuccess).toHaveBeenCalledWith("Default organization saved.");
    expect(refresh).toHaveBeenCalled();
  });

  it("says what auth refused, and claims nothing was saved", async () => {
    setDefaultOrganizationAction.mockResolvedValue({
      error: "you are not a member of this organization",
    });
    render(<DefaultOrganization organizations={ORGANIZATIONS} current="org_1" />);
    choose("org_2");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });

    expect(toastError).toHaveBeenCalledWith("you are not a member of this organization");
    expect(toastSuccess).not.toHaveBeenCalled();
    expect(refresh).not.toHaveBeenCalled();
  });

  it("reports a network error without claiming anything happened", async () => {
    setDefaultOrganizationAction.mockRejectedValue(new Error("offline"));
    render(<DefaultOrganization organizations={ORGANIZATIONS} current="org_1" />);
    choose("org_2");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });

    expect(toastError).toHaveBeenCalledWith("Network error. Please try again.");
    expect(refresh).not.toHaveBeenCalled();
  });
});
