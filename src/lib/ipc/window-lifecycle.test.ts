import { beforeEach, describe, expect, it, vi } from "vitest";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { protectDesktopClose } from "./window-lifecycle";

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(),
}));

describe("desktop close protection", () => {
  beforeEach(() => {
    vi.mocked(getCurrentWindow).mockReset();
  });

  it("prevents the native close until the draft flush succeeds", async () => {
    const destroy = vi.fn().mockResolvedValue(undefined);
    const onCloseRequested = vi.fn().mockResolvedValue(vi.fn());
    vi.mocked(getCurrentWindow).mockReturnValue({
      onCloseRequested,
      destroy,
    } as never);
    const flush = vi.fn().mockResolvedValue(undefined);
    const preventDefault = vi.fn();
    await protectDesktopClose(flush);

    const closeHandler = onCloseRequested.mock.calls[0]?.[0] as (event: {
      preventDefault: () => void;
    }) => void;
    closeHandler({ preventDefault });
    await vi.waitFor(() => expect(destroy).toHaveBeenCalledOnce());
    expect(preventDefault).toHaveBeenCalledOnce();
    expect(flush).toHaveBeenCalledOnce();
  });

  it("keeps the window open when the latest draft cannot be saved", async () => {
    const destroy = vi.fn().mockResolvedValue(undefined);
    const onCloseRequested = vi.fn().mockResolvedValue(vi.fn());
    vi.mocked(getCurrentWindow).mockReturnValue({
      onCloseRequested,
      destroy,
    } as never);
    await protectDesktopClose(vi.fn().mockRejectedValue(new Error("fictional save failure")));

    const closeHandler = onCloseRequested.mock.calls[0]?.[0] as (event: {
      preventDefault: () => void;
    }) => void;
    closeHandler({ preventDefault: vi.fn() });
    await Promise.resolve();
    await Promise.resolve();
    expect(destroy).not.toHaveBeenCalled();
  });
});
