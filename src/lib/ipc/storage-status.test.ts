import { beforeEach, describe, expect, it, vi } from "vitest";
import { storageStatus } from "./bindings";
import { decodeStorageCommandError, decodeStorageStatus, getStorageStatus } from "./storage-status";

vi.mock("./bindings", () => ({
  storageStatus: vi.fn(),
}));

const mockedStorageStatus = vi.mocked(storageStatus);

describe("storage_status 边界解码", () => {
  beforeEach(() => {
    mockedStorageStatus.mockReset();
  });

  it("接受完整的安全存储状态", () => {
    expect(
      decodeStorageStatus({
        ready: true,
        schemaVersion: 1,
        cipherAvailable: true,
        fts5Available: true,
        credentialStore: "windows",
      }),
    ).toEqual({
      ready: true,
      schemaVersion: 1,
      cipherAvailable: true,
      fts5Available: true,
      credentialStore: "windows",
    });
  });

  it.each([
    null,
    {},
    {
      ready: "yes",
      schemaVersion: 1,
      cipherAvailable: true,
      fts5Available: true,
      credentialStore: "windows",
    },
    {
      ready: true,
      schemaVersion: -1,
      cipherAvailable: true,
      fts5Available: true,
      credentialStore: "windows",
    },
    {
      ready: true,
      schemaVersion: 1.5,
      cipherAvailable: true,
      fts5Available: true,
      credentialStore: "windows",
    },
    {
      ready: true,
      schemaVersion: 4_294_967_296,
      cipherAvailable: true,
      fts5Available: true,
      credentialStore: "windows",
    },
    {
      ready: true,
      schemaVersion: 1,
      cipherAvailable: true,
      fts5Available: true,
      credentialStore: "file",
    },
  ])("拒绝无效状态载荷 %#", (payload) => {
    expect(() => decodeStorageStatus(payload)).toThrow(TypeError);
  });

  it("接受固定且不含内部诊断的命令错误", () => {
    expect(
      decodeStorageCommandError({
        code: "database_key_unavailable",
        message: "无法读取本地邮件数据库的安全密钥。",
        retryable: true,
      }),
    ).toEqual({
      code: "database_key_unavailable",
      message: "无法读取本地邮件数据库的安全密钥。",
      retryable: true,
    });
  });

  it.each([
    null,
    {},
    { code: "raw_sql_error", message: "C:\\secret\\mail.db", retryable: false },
    {
      code: "database_open_failed",
      message: "C:\\Users\\someone\\mail.db",
      retryable: true,
    },
    {
      code: "database_key_invalid",
      message: "本地邮件数据库密钥无效。",
      retryable: true,
    },
    { code: "internal", message: 500, retryable: true },
  ])("拒绝无效错误载荷 %#", (payload) => {
    expect(() => decodeStorageCommandError(payload)).toThrow(TypeError);
  });

  it("解码命令成功结果", async () => {
    mockedStorageStatus.mockResolvedValue({
      ready: true,
      schemaVersion: 1,
      cipherAvailable: true,
      fts5Available: true,
      credentialStore: "macos",
    });

    await expect(getStorageStatus()).resolves.toMatchObject({
      ready: true,
      credentialStore: "macos",
    });
  });

  it("保留命令拒绝，不伪造成功状态", async () => {
    const rejection = {
      code: "credential_store_unavailable",
      message: "系统凭据存储暂时不可用，请稍后重试。",
      retryable: true,
    };
    mockedStorageStatus.mockRejectedValue(rejection);

    await expect(getStorageStatus()).rejects.toBe(rejection);
  });
});
