import {
  securityDiagnostics,
  type Provider,
  type ProviderSecurityDiagnosticsV1,
  type SecurityDiagnosticsV1,
  type SecurityStorageDiagnosticsV1,
} from "./bindings";
import { hasExactKeys, isRecord, isUint32 } from "./decode";
import { isCredentialStoreKind, isStorageErrorCode } from "./storage-status";

export type {
  ProviderSecurityDiagnosticsV1,
  SecurityDiagnosticsV1,
  SecurityStorageDiagnosticsV1,
} from "./bindings";

const providerOrder: readonly Provider[] = ["gmail", "outlook", "qq", "netease"];
const rootKeys = ["appVersion", "platform", "online", "storage", "providers"] as const;
const storageKeys = [
  "ready",
  "schemaVersion",
  "cipherAvailable",
  "fts5Available",
  "credentialStore",
  "safeErrorCode",
] as const;
const providerKeys = [
  "provider",
  "configured",
  "accountCount",
  "connectedCount",
  "reconnectCount",
] as const;

function decodeStorage(value: unknown): SecurityStorageDiagnosticsV1 {
  if (!isRecord(value) || !hasExactKeys(value, storageKeys)) {
    throw new TypeError("security_diagnostics 返回了无效存储状态");
  }
  const { ready, schemaVersion, cipherAvailable, fts5Available, credentialStore, safeErrorCode } =
    value;
  if (
    typeof ready !== "boolean" ||
    (schemaVersion !== null && !isUint32(schemaVersion)) ||
    typeof cipherAvailable !== "boolean" ||
    typeof fts5Available !== "boolean" ||
    !isCredentialStoreKind(credentialStore) ||
    (safeErrorCode !== null && !isStorageErrorCode(safeErrorCode)) ||
    (ready && (schemaVersion === null || safeErrorCode !== null)) ||
    (!ready && (schemaVersion !== null || safeErrorCode === null))
  ) {
    throw new TypeError("security_diagnostics 返回了无效存储状态");
  }
  return {
    ready,
    schemaVersion,
    cipherAvailable,
    fts5Available,
    credentialStore,
    safeErrorCode,
  };
}

function decodeProvider(value: unknown, expectedProvider: Provider): ProviderSecurityDiagnosticsV1 {
  if (!isRecord(value) || !hasExactKeys(value, providerKeys)) {
    throw new TypeError("security_diagnostics 返回了无效服务商状态");
  }
  const { provider, configured, accountCount, connectedCount, reconnectCount } = value;
  if (provider !== expectedProvider || typeof configured !== "boolean") {
    throw new TypeError("security_diagnostics 返回了无效服务商状态");
  }
  if (accountCount === null && connectedCount === null && reconnectCount === null) {
    return {
      provider: expectedProvider,
      configured,
      accountCount,
      connectedCount,
      reconnectCount,
    };
  }
  if (!isUint32(accountCount) || !isUint32(connectedCount) || !isUint32(reconnectCount)) {
    throw new TypeError("security_diagnostics 返回了无效服务商状态");
  }
  if (connectedCount + reconnectCount > accountCount) {
    throw new TypeError("security_diagnostics 返回了无效服务商计数");
  }
  return {
    provider: expectedProvider,
    configured,
    accountCount,
    connectedCount,
    reconnectCount,
  };
}

export function decodeSecurityDiagnostics(value: unknown): SecurityDiagnosticsV1 {
  if (!isRecord(value) || !hasExactKeys(value, rootKeys)) {
    throw new TypeError("security_diagnostics 必须返回精确对象");
  }
  const { appVersion, platform, online, storage, providers } = value;
  if (
    typeof appVersion !== "string" ||
    appVersion.length === 0 ||
    typeof platform !== "string" ||
    platform.length === 0 ||
    typeof online !== "boolean" ||
    !Array.isArray(providers) ||
    providers.length !== providerOrder.length
  ) {
    throw new TypeError("security_diagnostics 返回了无效数据");
  }
  return {
    appVersion,
    platform,
    online,
    storage: decodeStorage(storage),
    providers: providers.map((provider, index) =>
      decodeProvider(provider, providerOrder[index] as Provider),
    ),
  };
}

export async function getSecurityDiagnostics(): Promise<SecurityDiagnosticsV1> {
  return decodeSecurityDiagnostics(await securityDiagnostics());
}
