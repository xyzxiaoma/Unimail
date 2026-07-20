import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cancelOAuthOnboarding,
  decodeOAuthOnboardingCommandError,
  getOAuthOnboardingStatus,
  startOAuthOnboarding,
  type ConnectedAccountSummary,
} from "../../lib/ipc/oauth-onboarding";
import { OAuthOnboardingDialog } from "./OAuthOnboardingDialog";

vi.mock("../../lib/ipc/oauth-onboarding", () => ({
  cancelOAuthOnboarding: vi.fn(),
  decodeOAuthOnboardingCommandError: vi.fn(),
  getOAuthOnboardingStatus: vi.fn(),
  startOAuthOnboarding: vi.fn(),
}));

const mockedCancel = vi.mocked(cancelOAuthOnboarding);
const mockedDecodeError = vi.mocked(decodeOAuthOnboardingCommandError);
const mockedGetStatus = vi.mocked(getOAuthOnboardingStatus);
const mockedStart = vi.mocked(startOAuthOnboarding);

const outlookAccount: ConnectedAccountSummary = {
  id: "account-2",
  provider: "outlook",
  email: "owner@outlook.example",
  displayName: "示例 Outlook 账户",
  authState: "connected",
};

describe("OAuthOnboardingDialog", () => {
  beforeEach(() => {
    mockedCancel.mockReset();
    mockedDecodeError.mockReset();
    mockedGetStatus.mockReset();
    mockedStart.mockReset();
    mockedGetStatus.mockImplementation((provider) =>
      Promise.resolve({
        provider,
        state: "idle",
        flowId: null,
        account: null,
        error: null,
      }),
    );
    mockedDecodeError.mockImplementation(() => {
      throw new TypeError("invalid test rejection");
    });
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("新账户可以选择 Outlook 并启动 provider-aware OAuth", async () => {
    mockedStart.mockResolvedValue({
      provider: "outlook",
      state: "waiting_for_browser",
      flowId: "flow-1",
      account: null,
      error: null,
    });
    render(
      <OAuthOnboardingDialog reconnectAccount={null} onClose={vi.fn()} onConnected={vi.fn()} />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Outlook" }));
    fireEvent.click(await screen.findByRole("button", { name: "连接 Outlook" }));
    await waitFor(() => expect(mockedStart).toHaveBeenCalledWith("outlook", null));
    expect(await screen.findByText("请在系统浏览器中继续")).toBeTruthy();
  });

  it("重新连接 Outlook 会传递提供商和账户 ID", async () => {
    mockedStart.mockResolvedValue({
      provider: "outlook",
      state: "waiting_for_browser",
      flowId: "flow-2",
      account: null,
      error: null,
    });
    render(
      <OAuthOnboardingDialog
        reconnectAccount={{ ...outlookAccount, authState: "needs_authentication" }}
        onClose={vi.fn()}
        onConnected={vi.fn()}
      />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "重新连接 Outlook" }));
    await waitFor(() => expect(mockedStart).toHaveBeenCalledWith("outlook", "account-2"));
  });

  it("等待状态会轮询，连接后停止并通知账户更新", async () => {
    vi.useFakeTimers();
    const onConnected = vi.fn();
    mockedGetStatus
      .mockResolvedValueOnce({
        provider: "outlook",
        state: "waiting_for_browser",
        flowId: "flow-1",
        account: null,
        error: null,
      })
      .mockResolvedValue({
        provider: "outlook",
        state: "connected",
        flowId: null,
        account: outlookAccount,
        error: null,
      });
    render(
      <OAuthOnboardingDialog
        initialProvider="outlook"
        reconnectAccount={null}
        onClose={vi.fn()}
        onConnected={onConnected}
      />,
    );

    await act(async () => Promise.resolve());
    await act(async () => vi.advanceTimersByTimeAsync(750));
    expect(screen.getByText("Outlook 已连接")).toBeTruthy();
    expect(onConnected).toHaveBeenCalledWith(outlookAccount);
  });

  it("Escape 会按当前提供商和 flow ID 取消并关闭", async () => {
    const onClose = vi.fn();
    mockedGetStatus.mockResolvedValue({
      provider: "outlook",
      state: "waiting_for_browser",
      flowId: "flow-current",
      account: null,
      error: null,
    });
    mockedCancel.mockResolvedValue({
      provider: "outlook",
      state: "cancelled",
      flowId: null,
      account: null,
      error: {
        provider: "outlook",
        code: "cancelled",
        message: "已取消 Outlook 连接。",
        retryable: true,
      },
    });
    render(
      <OAuthOnboardingDialog
        initialProvider="outlook"
        reconnectAccount={null}
        onClose={onClose}
        onConnected={vi.fn()}
      />,
    );
    expect(await screen.findByText("请在系统浏览器中继续")).toBeTruthy();

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(mockedCancel).toHaveBeenCalledWith("outlook", "flow-current"));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("不会显示未经验证的命令拒绝内容", async () => {
    const leakedUrl = "http://localhost/oauth/callback?code=secret";
    mockedStart.mockRejectedValue({ message: leakedUrl });
    render(
      <OAuthOnboardingDialog
        initialProvider="outlook"
        reconnectAccount={null}
        onClose={vi.fn()}
        onConnected={vi.fn()}
      />,
    );
    fireEvent.click(await screen.findByRole("button", { name: "连接 Outlook" }));

    expect(await screen.findByText("暂时无法读取 Outlook 连接状态。")).toBeTruthy();
    expect(screen.queryByText(leakedUrl)).toBeNull();
  });
});
