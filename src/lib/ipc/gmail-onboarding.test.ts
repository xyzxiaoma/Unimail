import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  cancelGmailOnboarding,
  connectedAccounts,
  gmailOnboardingStatus,
  startGmailOnboarding,
} from "./bindings";
import {
  decodeConnectedAccounts,
  decodeGmailOnboardingCommandError,
  decodeGmailOnboardingStatus,
  getConnectedAccounts,
  getGmailOnboardingStatus,
  startGmailOnboarding as startOnboarding,
} from "./gmail-onboarding";

vi.mock("./bindings", () => ({
  cancelGmailOnboarding: vi.fn(),
  connectedAccounts: vi.fn(),
  gmailOnboardingStatus: vi.fn(),
  startGmailOnboarding: vi.fn(),
}));

const mockedConnectedAccounts = vi.mocked(connectedAccounts);
const mockedStatus = vi.mocked(gmailOnboardingStatus);
const mockedStart = vi.mocked(startGmailOnboarding);

const account = {
  id: "account-1",
  provider: "gmail",
  email: "owner@example.com",
  displayName: "示例账户",
  authState: "connected",
} as const;

describe("Gmail onboarding IPC 边界", () => {
  beforeEach(() => {
    vi.mocked(cancelGmailOnboarding).mockReset();
    mockedConnectedAccounts.mockReset();
    mockedStatus.mockReset();
    mockedStart.mockReset();
  });

  it("解码安全状态与账户列表", () => {
    expect(
      decodeGmailOnboardingStatus({
        state: "connected",
        flowId: null,
        account,
        error: null,
      }),
    ).toEqual({ state: "connected", flowId: null, account, error: null });
    expect(decodeConnectedAccounts([account])).toEqual([account]);
  });

  it("只接受固定安全错误合同", () => {
    expect(
      decodeGmailOnboardingCommandError({
        code: "timed_out",
        message: "Gmail 授权已超时，请重试。",
        retryable: true,
      }),
    ).toEqual({
      code: "timed_out",
      message: "Gmail 授权已超时，请重试。",
      retryable: true,
    });
    expect(() =>
      decodeGmailOnboardingCommandError({
        code: "internal",
        message: "C:\\Users\\someone\\token.json",
        retryable: true,
      }),
    ).toThrow(TypeError);
  });

  it.each([
    null,
    {},
    { state: "waiting_for_browser", flowId: null, account: null, error: null },
    { state: "waiting_for_browser", flowId: "", account: null, error: null },
    { state: "connected", flowId: null, account: null, error: null },
    {
      state: "connected",
      flowId: null,
      account: { ...account, provider: "outlook" },
      error: null,
    },
    { state: "failed", flowId: null, account: null, error: null },
    { state: "idle", flowId: "stale-flow", account: null, error: null },
    { state: "secret_state", flowId: "flow", account: null, error: null },
  ])("拒绝不完整或未知状态 %#", (payload) => {
    expect(() => decodeGmailOnboardingStatus(payload)).toThrow(TypeError);
  });

  it("调用生成命令并保留参数", async () => {
    mockedStatus.mockResolvedValue({ state: "idle", flowId: null, account: null, error: null });
    mockedStart.mockResolvedValue({
      state: "waiting_for_browser",
      flowId: "flow-1",
      account: null,
      error: null,
    });
    mockedConnectedAccounts.mockResolvedValue([account]);

    await expect(getGmailOnboardingStatus()).resolves.toMatchObject({ state: "idle" });
    await expect(startOnboarding("account-1")).resolves.toMatchObject({
      state: "waiting_for_browser",
    });
    await expect(getConnectedAccounts()).resolves.toEqual([account]);
    expect(mockedStart).toHaveBeenCalledWith("account-1");
  });

  it("保留生成命令拒绝", async () => {
    const rejection = {
      code: "provider_unavailable",
      message: "暂时无法连接 Gmail，请稍后重试。",
      retryable: true,
    };
    mockedStart.mockRejectedValue(rejection);
    await expect(startOnboarding(null)).rejects.toBe(rejection);
  });
});
