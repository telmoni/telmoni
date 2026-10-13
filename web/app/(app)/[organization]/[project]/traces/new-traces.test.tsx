// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { NewTraces } from "@/components/traces/new-traces";
import { listDrawn, listIsEmpty, tracesArrived } from "@/lib/use-new-traces";

const mockRefresh = vi.fn();

vi.mock("next/navigation", () => ({
  useRouter: () => ({ refresh: mockRefresh }),
}));

const fresh = { rows: [], empty: false, first: true };

describe("NewTraces", () => {
  afterEach(() => {
    vi.clearAllMocks();
    for (const project of ["project_1", "project_2"]) listDrawn(project, fresh);
  });

  it("shows nothing until traces land, then counts them without drawing the list", () => {
    render(<NewTraces projectId="project_1" drawn="r1,r2" empty={false} />);
    expect(screen.queryByRole("status")).toBeNull();

    act(() => tracesArrived("project_1", ["a"], false));
    expect(screen.getByRole("status")).toHaveTextContent("1 new trace");
    act(() => tracesArrived("project_1", ["b", "c"], false));
    expect(screen.getByRole("status")).toHaveTextContent("3 new traces");
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("counts no run that the list shows already, whatever batches it still sends", () => {
    render(<NewTraces projectId="project_1" drawn="r1,r2" empty={false} />);
    act(() => tracesArrived("project_1", ["r1"], false));
    act(() => tracesArrived("project_1", ["r2", "r1"], false));
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("says the count is a floor when a nudge named only some of a batch", () => {
    render(<NewTraces projectId="project_1" drawn="r1" empty={false} />);
    act(() => tracesArrived("project_1", ["a"], true));
    expect(screen.getByRole("status")).toHaveTextContent("1+ new traces");
  });

  it("reads the list again on Show, and starts the count over", () => {
    render(<NewTraces projectId="project_1" drawn="r1" empty={false} />);
    act(() => tracesArrived("project_1", ["a", "b"], false));
    fireEvent.click(screen.getByRole("button", { name: "Show" }));
    expect(mockRefresh).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("takes the rows a redraw put on the list out of the count, and keeps the rest", () => {
    const { rerender } = render(<NewTraces projectId="project_1" drawn="r1" empty={false} />);
    act(() => tracesArrived("project_1", ["a", "b"], false));
    expect(screen.getByRole("status")).toHaveTextContent("2 new traces");

    rerender(<NewTraces projectId="project_1" drawn="a,r1" empty={false} />);
    expect(screen.getByRole("status"), "b was not drawn").toHaveTextContent("1 new trace");
    rerender(<NewTraces projectId="project_1" drawn="b,a,r1" empty={false} />);
    expect(screen.queryByRole("status")).toBeNull();
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("keeps the count through a render that drew the same rows", () => {
    const { rerender } = render(<NewTraces projectId="project_1" drawn="r1" empty={false} />);
    act(() => tracesArrived("project_1", ["a"], false));
    rerender(<NewTraces projectId="project_1" drawn="r1" empty={false} />);
    expect(screen.getByRole("status")).toHaveTextContent("1 new trace");
  });

  it("stands before nothing counted on a first draw, and tells the store whether it was empty", () => {
    tracesArrived("project_1", ["left", "over"], false);
    render(<NewTraces projectId="project_1" drawn="" empty />);
    expect(screen.queryByRole("status")).toBeNull();
    expect(listIsEmpty("project_1")).toBe(true);

    act(() => tracesArrived("project_1", ["a"], false));
    expect(screen.queryByRole("status"), "an empty list shows no bar; the listener draws it").toBeNull();
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("counts another project's traces for no one here", () => {
    render(<NewTraces projectId="project_1" drawn="r1" empty={false} />);
    act(() => tracesArrived("project_2", ["a"], false));
    expect(screen.queryByRole("status")).toBeNull();
  });
});
