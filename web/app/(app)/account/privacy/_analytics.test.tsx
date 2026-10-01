// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const refresh = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", () => ({
  useRouter: () => ({ refresh }),
}));

const toastError = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({ toast: { error: toastError, success: vi.fn() } }));

const setAnalyticsPreferenceAction = vi.hoisted(() =>
  vi.fn<(optIn: boolean) => Promise<{ error: string | null }>>(async () => ({
    error: null,
  })),
);
vi.mock("./actions", () => ({ setAnalyticsPreferenceAction }));

import { AnalyticsPreference } from "./_analytics";

beforeEach(() => {
  vi.clearAllMocks();
  setAnalyticsPreferenceAction.mockResolvedValue({ error: null });
});

const toggle = () => screen.getByRole("switch");

describe("AnalyticsPreference", () => {
  it("draws the stored consent, off by default", () => {
    const { unmount } = render(<AnalyticsPreference optIn={false} />);
    expect(toggle()).not.toBeChecked();
    unmount();

    render(<AnalyticsPreference optIn />);
    expect(toggle()).toBeChecked();
  });

  it("sends the consent the press means", async () => {
    render(<AnalyticsPreference optIn={false} />);
    await act(async () => {
      fireEvent.click(toggle());
    });
    expect(setAnalyticsPreferenceAction).toHaveBeenCalledWith(true);

    await act(async () => {
      fireEvent.click(toggle());
    });
    expect(setAnalyticsPreferenceAction).toHaveBeenLastCalledWith(false);
  });

  it("flips optimistically and refreshes once the write lands", async () => {
    render(<AnalyticsPreference optIn={false} />);
    await act(async () => {
      fireEvent.click(toggle());
    });
    expect(toggle()).toBeChecked();
    expect(refresh).toHaveBeenCalled();
  });

  it("puts the switch back when the service refuses", async () => {
    setAnalyticsPreferenceAction.mockResolvedValue({
      error: "this preference is your own",
    });
    render(<AnalyticsPreference optIn={false} />);
    await act(async () => {
      fireEvent.click(toggle());
    });

    expect(toggle()).not.toBeChecked();
    expect(toastError).toHaveBeenCalledWith("this preference is your own");
    expect(refresh).not.toHaveBeenCalled();
  });

  it("puts it back on a network error too", async () => {
    setAnalyticsPreferenceAction.mockRejectedValue(new Error("offline"));
    render(<AnalyticsPreference optIn />);
    await act(async () => {
      fireEvent.click(toggle());
    });

    expect(toggle()).toBeChecked();
    expect(toastError).toHaveBeenCalledWith("Network error. Please try again.");
  });

  it("says it is off by default, what it sends, and what it never sends", () => {
    render(<AnalyticsPreference optIn={false} />);
    const body = document.body.textContent ?? "";
    expect(body, "the copy does not say it is off until asked").toMatch(
      /off unless you turn it on/i,
    );
    expect(body).toMatch(/analytics provider/i);
    expect(body).toMatch(/still writes its own operational log/i);
    expect(body).toMatch(/never includes/i);
  });
});
