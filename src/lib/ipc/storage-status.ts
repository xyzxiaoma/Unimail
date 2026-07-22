import {
  storageStatus,
  type CredentialStoreKind,
  type StorageCommandError,
  type StorageErrorCode,
  type StorageStatus,
} from "./bindings";
import { isRecord } from "./decode";

export type { StorageCommandError, StorageStatus } from "./bindings";

const credentialStoreKinds = new Set<CredentialStoreKind>(["windows", "macos", "unsupported"]);

const storageErrorContracts: Record<
  StorageErrorCode,
  Pick<StorageCommandError, "message" | "retryable">
> = {
  credential_store_unavailable: {
    message: "系统凭据存储暂时不可用，请稍后重试。",
    retryable: true,
  },
  database_key_unavailable: {
    message: "无法读取本地邮件数据库的安全密钥。",
    retryable: true,
  },
  database_key_invalid: { message: "本地邮件数据库密钥无效。", retryable: false },
  database_open_failed: { message: "无法打开本地邮件数据库。", retryable: true },
  cipher_unavailable: { message: "当前安装缺少加密数据库能力。", retryable: false },
  fts5_unavailable: { message: "当前安装缺少邮件搜索能力。", retryable: false },
  migration_failed: { message: "本地邮件数据库升级失败。", retryable: false },
  storage_busy: { message: "本地邮件存储正忙，请稍后重试。", retryable: true },
  not_found: { message: "未找到请求的本地邮件数据。", retryable: false },
  revision_conflict: { message: "内容已在其他位置更新，请刷新后重试。", retryable: false },
  constraint_violation: { message: "请求的数据与本地邮件存储规则冲突。", retryable: false },
  invalid_data: { message: "本地邮件数据格式无效。", retryable: false },
  cleanup_pending: { message: "本地账户数据仍在清理中，请稍后重试。", retryable: true },
  internal: { message: "本地邮件存储发生错误，请稍后重试。", retryable: true },
};

const storageErrorCodes = new Set<StorageErrorCode>(
  Object.keys(storageErrorContracts) as StorageErrorCode[],
);

export function isCredentialStoreKind(value: unknown): value is CredentialStoreKind {
  return typeof value === "string" && credentialStoreKinds.has(value as CredentialStoreKind);
}

export function isStorageErrorCode(value: unknown): value is StorageErrorCode {
  return typeof value === "string" && storageErrorCodes.has(value as StorageErrorCode);
}

export function decodeStorageStatus(value: unknown): StorageStatus {
  if (!isRecord(value)) {
    throw new TypeError("storage_status 必须返回对象");
  }

  const { ready, schemaVersion, cipherAvailable, fts5Available, credentialStore } = value;
  if (
    typeof ready !== "boolean" ||
    typeof schemaVersion !== "number" ||
    !Number.isInteger(schemaVersion) ||
    schemaVersion < 0 ||
    schemaVersion > 4_294_967_295 ||
    typeof cipherAvailable !== "boolean" ||
    typeof fts5Available !== "boolean" ||
    !isCredentialStoreKind(credentialStore)
  ) {
    throw new TypeError("storage_status 返回了无效数据");
  }

  return { ready, schemaVersion, cipherAvailable, fts5Available, credentialStore };
}

export function decodeStorageCommandError(value: unknown): StorageCommandError {
  if (!isRecord(value)) {
    throw new TypeError("storage_status 错误必须为对象");
  }

  const { code, message, retryable } = value;
  if (!isStorageErrorCode(code)) {
    throw new TypeError("storage_status 返回了无效错误");
  }
  const contract = storageErrorContracts[code];
  if (message !== contract.message || retryable !== contract.retryable) {
    throw new TypeError("storage_status 返回了无效错误");
  }

  return { code, message, retryable };
}

export async function getStorageStatus(): Promise<StorageStatus> {
  return decodeStorageStatus(await storageStatus());
}
