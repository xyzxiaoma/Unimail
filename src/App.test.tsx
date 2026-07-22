import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { getConnectedAccounts, getOAuthOnboardingStatus } from "./lib/ipc/oauth-onboarding";
import { decodeStorageCommandError, getStorageStatus } from "./lib/ipc/storage-status";

vi.mock("./lib/ipc/application-info", () => ({
  getApplicationInfo: vi.fn().mockRejectedValue(new Error("IPC unavailable in test")),
}));

vi.mock("./lib/ipc/storage-status", () => ({
  decodeStorageCommandError: vi.fn(),
  getStorageStatus: vi.fn(),
}));

vi.mock("./lib/ipc/oauth-onboarding", () => ({
  cancelOAuthOnboarding: vi.fn(),
  decodeOAuthOnboardingCommandError: vi.fn(() => {
    throw new TypeError("invalid test rejection");
  }),
  getConnectedAccounts: vi.fn(),
  getOAuthOnboardingStatus: vi.fn(),
  startOAuthOnboarding: vi.fn(),
}));

vi.mock("./lib/ipc/mail-reader", () => ({
  getInboxPage: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
  getMailMessageDetail: vi.fn(),
  setMailMessageRead: vi.fn(),
}));

vi.mock("./lib/ipc/compose", () => ({
  createLocalReplyDraft: vi.fn(),
  decodeComposeCommandError: vi.fn(() => {
    throw new TypeError("invalid test rejection");
  }),
  getDrafts: vi.fn().mockResolvedValue([]),
  getLocalDraft: vi.fn(),
  getSentItems: vi.fn().mockResolvedValue([]),
  permitOutboundRetry: vi.fn(),
  refreshLocalSentItems: vi.fn(),
  removeLocalDraft: vi.fn(),
  reportDesktopConnectivity: vi.fn().mockResolvedValue(undefined),
  saveLocalDraft: vi.fn(),
  submitLocalDraft: vi.fn(),
}));

const mockedDecodeStorageCommandError = vi.mocked(decodeStorageCommandError);
const mockedGetStorageStatus = vi.mocked(getStorageStatus);
const mockedGetConnectedAccounts = vi.mocked(getConnectedAccounts);
const mockedGetOAuthOnboardingStatus = vi.mocked(getOAuthOnboardingStatus);

function renderApp() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>,
  );
}

describe("Unimail 基础界面", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
    mockedGetStorageStatus.mockReset();
    mockedGetStorageStatus.mockRejectedValue(new Error("IPC unavailable in test"));
    mockedDecodeStorageCommandError.mockReset();
    mockedDecodeStorageCommandError.mockImplementation(() => {
      throw new TypeError("invalid test rejection");
    });
    mockedGetConnectedAccounts.mockReset();
    mockedGetConnectedAccounts.mockResolvedValue([]);
    mockedGetOAuthOnboardingStatus.mockReset();
    mockedGetOAuthOnboardingStatus.mockImplementation((provider) =>
      Promise.resolve({
        provider,
        state: "idle",
        flowId: null,
        account: null,
        error: null,
      }),
    );
  });

  afterEach(() => {
    cleanup();
  });

  it("展示中文三栏空状态和桌面状态占位", async () => {
    renderApp();

    expect(screen.getByRole("navigation", { name: "邮件文件夹" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "收件箱", level: 1 })).toBeTruthy();
    expect(await screen.findByText("收件箱空空如也")).toBeTruthy();
    expect(screen.getByText("选择一封邮件开始阅读")).toBeTruthy();
    expect(screen.getByText("无法读取加密存储状态")).toBeTruthy();
    expect(screen.getByText("等待添加账户")).toBeTruthy();
  });

  it("可通过按钮打开真实写信表单并用 Escape 关闭", async () => {
    renderApp();

    fireEvent.click(screen.getByRole("button", { name: /写邮件/ }));
    expect(screen.getByRole("dialog", { name: "撰写邮件" })).toBeTruthy();
    expect(screen.getByPlaceholderText("name@example.com，多个地址用逗号分隔")).toBeTruthy();
    expect(screen.getByPlaceholderText("邮件主题")).toBeTruthy();
    expect(screen.getByPlaceholderText("输入邮件正文")).toBeTruthy();

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "撰写邮件" })).toBeNull());
  });

  it("草稿与已发送侧栏入口切换到真实固定视图", async () => {
    renderApp();

    fireEvent.click(screen.getByRole("button", { name: "草稿" }));
    expect(await screen.findByRole("heading", { name: "草稿", level: 1 })).toBeTruthy();
    expect(screen.getByText("还没有草稿")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "已发送" }));
    expect(await screen.findByRole("heading", { name: "已发送", level: 1 })).toBeTruthy();
    expect(await screen.findByText("还没有发送记录")).toBeTruthy();
  });

  it("同步占位会提供非破坏性的状态反馈", () => {
    renderApp();

    fireEvent.click(screen.getByRole("button", { name: "同步邮件" }));
    expect(screen.getByText("尚无可同步账户")).toBeTruthy();
  });

  it("授权失效的 Gmail 账户保留重新连接入口", async () => {
    mockedGetConnectedAccounts.mockResolvedValue([
      {
        id: "gmail-needs-auth",
        provider: "gmail",
        email: "owner@example.com",
        displayName: null,
        authState: "needs_authentication",
      },
    ]);

    renderApp();

    expect(await screen.findByText("Gmail 账户需要重新连接")).toBeTruthy();
    expect(screen.getByText("重新连接 Gmail")).toBeTruthy();
  });

  it("两个添加账户入口都打开 邮箱连接对话框", async () => {
    const { unmount } = renderApp();

    fireEvent.click(screen.getByRole("button", { name: "开始设置" }));
    expect(await screen.findByRole("dialog", { name: "连接 Gmail" })).toBeTruthy();
    unmount();

    renderApp();
    fireEvent.click(await screen.findByRole("button", { name: "添加邮箱账户" }));
    expect(await screen.findByRole("dialog", { name: "连接 Gmail" })).toBeTruthy();
  });

  it("邮箱连接对话框响应 Escape 并把焦点还给入口", async () => {
    renderApp();
    const opener = screen.getByRole("button", { name: "开始设置" });
    fireEvent.click(opener);
    expect(await screen.findByRole("dialog", { name: "连接 Gmail" })).toBeTruthy();

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "连接 Gmail" })).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(opener));
  });

  it("邮箱连接对话框打开时不会响应写邮件快捷键", async () => {
    renderApp();
    fireEvent.click(screen.getByRole("button", { name: "开始设置" }));
    expect(await screen.findByRole("dialog", { name: "连接 Gmail" })).toBeTruthy();

    fireEvent.keyDown(window, { key: "n" });
    expect(screen.queryByRole("dialog", { name: "撰写邮件" })).toBeNull();
  });

  it("展示经过解码的加密存储就绪状态", async () => {
    mockedGetStorageStatus.mockResolvedValue({
      ready: true,
      schemaVersion: 1,
      cipherAvailable: true,
      fts5Available: true,
      credentialStore: "windows",
    });

    renderApp();

    expect(await screen.findByText("加密存储已就绪 · Schema 1")).toBeTruthy();
  });

  it("不会把未经验证的命令拒绝内容显示给用户", async () => {
    const leakedPath = "C:\\Users\\someone\\mail.db";
    mockedGetStorageStatus.mockRejectedValue({
      code: "database_open_failed",
      message: leakedPath,
      retryable: true,
    });

    renderApp();

    expect(await screen.findByText("无法读取加密存储状态")).toBeTruthy();
    expect(screen.queryByText(leakedPath)).toBeNull();
  });
});
