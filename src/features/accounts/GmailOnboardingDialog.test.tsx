import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cancelGmailOnboarding,
  decodeGmailOnboardingCommandError,
  getGmailOnboardingStatus,
  startGmailOnboarding,
  type ConnectedAccountSummary,
} from "../../lib/ipc/gmail-onboarding";
import { GmailOnboardingDialog } from "./GmailOnboardingDialog";

vi.mock("../../lib/ipc/gmail-onboarding", () => ({
  cancelGmailOnboarding: vi.fn(),
  decodeGmailOnboardingCommandError: vi.fn(),
  getGmailOnboardingStatus: vi.fn(),
  startGmailOnboarding: vi.fn(),
}));

const mockedCancel = vi.mocked(cancelGmailOnboarding);
const mockedDecodeError = vi.mocked(decodeGmailOnboardingCommandError);
const mockedGetStatus = vi.mocked(getGmailOnboardingStatus);
const mockedStart = vi.mocked(startGmailOnboarding);

const account: ConnectedAccountSummary = {
  id: "account-1",
  provider: "gmail",
  email: "owner@example.com",
  displayName: "示例账户",
  authState: "connected",
};

describe("GmailOnboardingDialog", () => {
  beforeEach(() => {
    mockedCancel.mockReset();
    mockedDecodeError.mockReset();
    mockedGetStatus.mockReset();
    mockedStart.mockReset();
    mockedGetStatus.mockResolvedValue({
      state: "idle",
      flowId: null,
      account: null,
      error: null,
    });
    mockedDecodeError.mockImplementation(() => {
      throw new TypeError("invalid test rejection");
    });
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("未配置构建显示安全说明且不提供连接动作", async () => {
    mockedGetStatus.mockResolvedValue({
      state: "unconfigured",
      flowId: null,
      account: null,
      error: {
        code: "not_configured",
        message: "当前构建未配置 Gmail 接入。",
        retryable: false,
      },
    });

    render(
      <GmailOnboardingDialog reconnectAccount={null} onClose={vi.fn()} onConnected={vi.fn()} />,
    );

    expect(await screen.findByText("未配置 Gmail 接入")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "连接 Gmail" })).toBeNull();
  });

  it("重新连接会传递账户 ID", async () => {
    mockedStart.mockResolvedValue({
      state: "waiting_for_browser",
      flowId: "flow-1",
      account: null,
      error: null,
    });

    render(
      <GmailOnboardingDialog
        reconnectAccount={{ ...account, authState: "needs_authentication" }}
        onClose={vi.fn()}
        onConnected={vi.fn()}
      />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "重新连接 Gmail" }));
    await waitFor(() => expect(mockedStart).toHaveBeenCalledWith("account-1"));
    expect(await screen.findByText("请在系统浏览器中继续")).toBeTruthy();
  });

  it("已有成功终态时仍可再次连接当前账户", async () => {
    mockedGetStatus.mockResolvedValue({
      state: "connected",
      flowId: null,
      account,
      error: null,
    });
    mockedStart.mockResolvedValue({
      state: "waiting_for_browser",
      flowId: "flow-reconnect",
      account: null,
      error: null,
    });

    render(
      <GmailOnboardingDialog reconnectAccount={account} onClose={vi.fn()} onConnected={vi.fn()} />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "重新连接 Gmail" }));
    await waitFor(() => expect(mockedStart).toHaveBeenCalledWith("account-1"));
    expect(await screen.findByText("请在系统浏览器中继续")).toBeTruthy();
  });

  it("等待状态会轮询，连接后停止并通知账户更新", async () => {
    vi.useFakeTimers();
    const onConnected = vi.fn();
    mockedGetStatus
      .mockResolvedValueOnce({
        state: "waiting_for_browser",
        flowId: "flow-1",
        account: null,
        error: null,
      })
      .mockResolvedValue({
        state: "connected",
        flowId: null,
        account,
        error: null,
      });

    render(
      <GmailOnboardingDialog reconnectAccount={null} onClose={vi.fn()} onConnected={onConnected} />,
    );

    await act(async () => Promise.resolve());
    expect(screen.getByText("请在系统浏览器中继续")).toBeTruthy();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(750);
    });
    expect(screen.getByText("Gmail 已连接")).toBeTruthy();
    expect(onConnected).toHaveBeenCalledWith(account);
  });

  it("Escape 会用当前 flow ID 取消并关闭", async () => {
    const onClose = vi.fn();
    mockedGetStatus.mockResolvedValue({
      state: "waiting_for_browser",
      flowId: "flow-current",
      account: null,
      error: null,
    });
    mockedCancel.mockResolvedValue({
      state: "cancelled",
      flowId: null,
      account: null,
      error: {
        code: "cancelled",
        message: "已取消 Gmail 连接。",
        retryable: true,
      },
    });

    render(
      <GmailOnboardingDialog reconnectAccount={null} onClose={onClose} onConnected={vi.fn()} />,
    );
    expect(await screen.findByText("请在系统浏览器中继续")).toBeTruthy();

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(mockedCancel).toHaveBeenCalledWith("flow-current"));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("不会显示未经验证的命令拒绝内容", async () => {
    const leakedUrl = "http://127.0.0.1/oauth/callback?code=secret";
    mockedStart.mockRejectedValue({ message: leakedUrl });

    render(
      <GmailOnboardingDialog reconnectAccount={null} onClose={vi.fn()} onConnected={vi.fn()} />,
    );
    fireEvent.click(await screen.findByRole("button", { name: "连接 Gmail" }));

    expect(await screen.findByText("暂时无法读取 Gmail 连接状态。")).toBeTruthy();
    expect(screen.queryByText(leakedUrl)).toBeNull();
  });

  it("启动命令返回前禁止关闭，避免遗留孤儿授权流程", async () => {
    let resolveStart!: (value: Awaited<ReturnType<typeof startGmailOnboarding>>) => void;
    mockedStart.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveStart = resolve;
        }),
    );
    const onClose = vi.fn();

    render(
      <GmailOnboardingDialog reconnectAccount={null} onClose={onClose} onConnected={vi.fn()} />,
    );
    fireEvent.click(await screen.findByRole("button", { name: "连接 Gmail" }));
    fireEvent.keyDown(window, { key: "Escape" });

    expect(screen.getByRole("button", { name: "关闭 Gmail 连接窗口" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "关闭" })).toBeDisabled();
    expect(onClose).not.toHaveBeenCalled();

    resolveStart({
      state: "waiting_for_browser",
      flowId: "flow-1",
      account: null,
      error: null,
    });
    await waitFor(() => expect(screen.getByText("请在系统浏览器中继续")).toBeTruthy());
  });

  it("从标题按 Tab 或 Shift+Tab 都会把焦点留在对话框内", async () => {
    render(
      <GmailOnboardingDialog reconnectAccount={null} onClose={vi.fn()} onConnected={vi.fn()} />,
    );
    const title = await screen.findByRole("heading", { name: "连接 Gmail", level: 2 });
    expect(title).toHaveFocus();

    fireEvent.keyDown(window, { key: "Tab" });
    expect(screen.getByRole("button", { name: "关闭 Gmail 连接窗口" })).toHaveFocus();

    title.focus();
    fireEvent.keyDown(window, { key: "Tab", shiftKey: true });
    expect(screen.getByRole("button", { name: "连接 Gmail" })).toHaveFocus();
  });
});
