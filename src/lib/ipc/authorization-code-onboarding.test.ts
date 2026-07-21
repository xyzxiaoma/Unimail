import { beforeEach, describe, expect, it, vi } from "vitest";
import { connectAuthorizationCodeAccount as command } from "./bindings";
import { connectAuthorizationCodeAccount } from "./authorization-code-onboarding";

vi.mock("./bindings", () => ({
  connectAuthorizationCodeAccount: vi.fn(),
}));

const mockedCommand = vi.mocked(command);

describe("授权码 onboarding IPC 边界", () => {
  beforeEach(() => mockedCommand.mockReset());

  it("传递授权码命令并只返回安全账户摘要", async () => {
    mockedCommand.mockResolvedValue({
      id: "account-qq",
      provider: "qq",
      email: "owner@qq.com",
      displayName: null,
      authState: "connected",
    });
    await expect(
      connectAuthorizationCodeAccount("qq", null, "owner@qq.com", "private-code"),
    ).resolves.toMatchObject({ provider: "qq", email: "owner@qq.com" });
    expect(mockedCommand).toHaveBeenCalledWith("qq", null, "owner@qq.com", "private-code");
  });

  it("拒绝包含授权码字段的伪造响应", async () => {
    mockedCommand.mockResolvedValue({
      id: "account-qq",
      provider: "qq",
      email: "owner@qq.com",
      displayName: null,
      authState: "connected",
      authorizationCode: "private-code",
    });
    await expect(
      connectAuthorizationCodeAccount("qq", null, "owner@qq.com", "private-code"),
    ).resolves.not.toHaveProperty("authorizationCode");
  });
});
