import { describe, expect, it, vi } from "vitest";
import { securityDiagnostics } from "./bindings";
import { decodeSecurityDiagnostics, getSecurityDiagnostics } from "./security-diagnostics";

vi.mock("./bindings", () => ({ securityDiagnostics: vi.fn() }));

const valid = {
  appVersion: "0.1.0",
  platform: "windows",
  online: true,
  storage: {
    ready: true,
    schemaVersion: 4,
    cipherAvailable: true,
    fts5Available: true,
    credentialStore: "windows",
    safeErrorCode: null,
  },
  providers: [
    {
      provider: "gmail",
      configured: true,
      accountCount: 2,
      connectedCount: 1,
      reconnectCount: 1,
    },
    {
      provider: "outlook",
      configured: false,
      accountCount: 0,
      connectedCount: 0,
      reconnectCount: 0,
    },
    {
      provider: "qq",
      configured: true,
      accountCount: 1,
      connectedCount: 1,
      reconnectCount: 0,
    },
    {
      provider: "netease",
      configured: true,
      accountCount: 0,
      connectedCount: 0,
      reconnectCount: 0,
    },
  ],
} as const;

describe("security_diagnostics 边界解码", () => {
  it("接受精确的无隐私诊断合同", async () => {
    expect(decodeSecurityDiagnostics(valid)).toEqual(valid);
    vi.mocked(securityDiagnostics).mockResolvedValueOnce(valid);
    await expect(getSecurityDiagnostics()).resolves.toEqual(valid);
  });

  it("接受存储不可用和全部未知的账户计数", () => {
    expect(
      decodeSecurityDiagnostics({
        ...valid,
        storage: {
          ...valid.storage,
          ready: false,
          schemaVersion: null,
          cipherAvailable: false,
          fts5Available: false,
          safeErrorCode: "database_open_failed",
        },
        providers: valid.providers.map((provider) => ({
          ...provider,
          accountCount: null,
          connectedCount: null,
          reconnectCount: null,
        })),
      }).providers[0]?.accountCount,
    ).toBeNull();
  });

  it.each([
    null,
    { ...valid, appVersion: "" },
    { ...valid, unknown: "value" },
    { ...valid, storage: { ...valid.storage, databasePath: "C:/private" } },
    {
      ...valid,
      providers: [{ ...valid.providers[0], accountId: "private" }, ...valid.providers.slice(1)],
    },
    { ...valid, providers: [...valid.providers].reverse() },
    {
      ...valid,
      providers: [
        { ...valid.providers[0], connectedCount: 2, reconnectCount: 1 },
        ...valid.providers.slice(1),
      ],
    },
    {
      ...valid,
      providers: [{ ...valid.providers[0], accountCount: null }, ...valid.providers.slice(1)],
    },
    { ...valid, storage: { ...valid.storage, ready: false } },
  ])("拒绝越界、混合或附加敏感字段的载荷 %#", (payload) => {
    expect(() => decodeSecurityDiagnostics(payload)).toThrow(TypeError);
  });
});
