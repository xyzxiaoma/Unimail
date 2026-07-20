import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  cancelOauthOnboarding,
  connectedAccounts,
  oauthOnboardingStatus,
  startOauthOnboarding,
} from "./bindings";
import {
  decodeConnectedAccounts,
  decodeOAuthOnboardingCommandError,
  decodeOAuthOnboardingStatus,
  getConnectedAccounts,
  getOAuthOnboardingStatus,
  startOAuthOnboarding,
} from "./oauth-onboarding";

vi.mock("./bindings", () => ({
  cancelOauthOnboarding: vi.fn(),
  connectedAccounts: vi.fn(),
  oauthOnboardingStatus: vi.fn(),
  startOauthOnboarding: vi.fn(),
}));

const mockedConnectedAccounts = vi.mocked(connectedAccounts);
const mockedStatus = vi.mocked(oauthOnboardingStatus);
const mockedStart = vi.mocked(startOauthOnboarding);

const gmailAccount = {
  id: "account-1",
  provider: "gmail",
  email: "owner@example.com",
  displayName: "示例账户",
  authState: "connected",
} as const;

const outlookAccount = {
  ...gmailAccount,
  id: "account-2",
  provider: "outlook",
  email: "owner@outlook.example",
} as const;

describe("OAuth onboarding IPC 边界", () => {
  beforeEach(() => {
    vi.mocked(cancelOauthOnboarding).mockReset();
    mockedConnectedAccounts.mockReset();
    mockedStatus.mockReset();
    mockedStart.mockReset();
  });

  it("解码 Gmail 和 Outlook 安全状态与账户列表", () => {
    expect(
      decodeOAuthOnboardingStatus({
        provider: "outlook",
        state: "connected",
        flowId: null,
        account: outlookAccount,
        error: null,
      }),
    ).toEqual({
      provider: "outlook",
      state: "connected",
      flowId: null,
      account: outlookAccount,
      error: null,
    });
    expect(decodeConnectedAccounts([gmailAccount, outlookAccount])).toEqual([
      gmailAccount,
      outlookAccount,
    ]);
  });

  it("只接受与提供商匹配的固定安全错误合同", () => {
    expect(
      decodeOAuthOnboardingCommandError({
        provider: "outlook",
        code: "timed_out",
        message: "Outlook 授权已超时，请重试。",
        retryable: true,
      }),
    ).toMatchObject({ provider: "outlook", code: "timed_out" });
    expect(() =>
      decodeOAuthOnboardingCommandError({
        provider: "outlook",
        code: "internal",
        message: "C:\\Users\\someone\\token.json",
        retryable: true,
      }),
    ).toThrow(TypeError);
  });

  it.each([
    null,
    {},
    { provider: "outlook", state: "waiting_for_browser", flowId: null, account: null, error: null },
    { provider: "outlook", state: "connected", flowId: null, account: gmailAccount, error: null },
    { provider: "qq", state: "idle", flowId: null, account: null, error: null },
    { provider: "gmail", state: "failed", flowId: null, account: null, error: null },
  ])("拒绝不完整、错配或不支持的状态 %#", (payload) => {
    expect(() => decodeOAuthOnboardingStatus(payload)).toThrow(TypeError);
  });

  it("调用 provider-aware 生成命令并保留参数", async () => {
    mockedStatus.mockResolvedValue({
      provider: "outlook",
      state: "idle",
      flowId: null,
      account: null,
      error: null,
    });
    mockedStart.mockResolvedValue({
      provider: "outlook",
      state: "waiting_for_browser",
      flowId: "flow-1",
      account: null,
      error: null,
    });
    mockedConnectedAccounts.mockResolvedValue([gmailAccount, outlookAccount]);

    await expect(getOAuthOnboardingStatus("outlook")).resolves.toMatchObject({ state: "idle" });
    await expect(startOAuthOnboarding("outlook", "account-2")).resolves.toMatchObject({
      state: "waiting_for_browser",
    });
    await expect(getConnectedAccounts()).resolves.toHaveLength(2);
    expect(mockedStatus).toHaveBeenCalledWith("outlook");
    expect(mockedStart).toHaveBeenCalledWith("outlook", "account-2");
  });
});
