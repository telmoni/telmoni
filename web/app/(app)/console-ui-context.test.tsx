// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const push = vi.fn();
vi.mock("next/navigation", () => ({
  usePathname: () => "/acme",
  useRouter: () => ({ push }),
}));

const info = vi.fn();
vi.mock("sonner", () => ({ toast: { info: (...a: unknown[]) => info(...a) } }));

let ceiling: ProjectCeiling | null = null;
vi.mock("@/lib/store", () => ({ useProjectCeiling: () => ceiling }));

import { ConsoleUiProvider, useConsoleUi } from "@/components/console-ui-context";
import type { ProjectCeiling } from "@/lib/server/entities/projects";

const DETAIL = "This organization is on Hobby, which holds 10 projects.";

function Opener() {
  const { creating, openCreate } = useConsoleUi();
  return (
    <>
      <button type="button" onClick={() => openCreate("project")}>
        New project
      </button>
      <button type="button" onClick={() => openCreate("organization")}>
        New organization
      </button>
      <span data-testid="creating">{creating?.kind ?? "none"}</span>
    </>
  );
}

function newProject() {
  render(
    <ConsoleUiProvider>
      <Opener />
    </ConsoleUiProvider>,
  );
  fireEvent.click(screen.getByRole("button", { name: "New project" }));
}

beforeEach(() => {
  vi.clearAllMocks();
  ceiling = null;
});

describe("New project at the plan's ceiling", () => {
  it("goes to where the ceiling is lifted, saying why, instead of opening its panel", () => {
    ceiling = { limit: 10, held: 10, reached: true, detail: DETAIL, page: "/acme/billing" };
    newProject();
    expect(push).toHaveBeenCalledWith("/acme/billing");
    expect(info).toHaveBeenCalledWith(DETAIL);
    expect(screen.getByTestId("creating")).toHaveTextContent("none");
  });

  it("says why and stays where it is when no page lifts it", () => {
    ceiling = { limit: 10, held: 10, reached: true, detail: DETAIL, page: null };
    newProject();
    expect(push).not.toHaveBeenCalled();
    expect(info).toHaveBeenCalledWith(DETAIL);
    expect(screen.getByTestId("creating")).toHaveTextContent("none");
  });

  it("opens the panel below the ceiling, and where none is held", () => {
    ceiling = { limit: 10, held: 9, reached: false, detail: DETAIL, page: "/acme/billing" };
    newProject();
    expect(screen.getByTestId("creating")).toHaveTextContent("project");
    expect(push).not.toHaveBeenCalled();
  });

  it("never holds back a new organization", () => {
    ceiling = { limit: 10, held: 10, reached: true, detail: DETAIL, page: "/acme/billing" };
    newProject();
    fireEvent.click(screen.getByRole("button", { name: "New organization" }));
    expect(screen.getByTestId("creating")).toHaveTextContent("organization");
  });
});
