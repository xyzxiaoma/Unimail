import {
  credentialStoreLabels,
  providerLabels,
  securityDiagnosticsContent as content,
  storageStatusLabels,
} from "../../content/security-diagnostics.zh-CN";
import type { SecurityDiagnosticsV1 } from "../../lib/ipc/security-diagnostics";

function countLabel(value: number | null): string {
  return value === null ? content.unavailable : String(value);
}

export function formatSecurityDiagnostics(diagnostics: SecurityDiagnosticsV1): string {
  const storageStatus = diagnostics.storage.safeErrorCode
    ? storageStatusLabels[diagnostics.storage.safeErrorCode]
    : content.none;
  const lines = [
    `${content.labels.appVersion}：${diagnostics.appVersion}`,
    `${content.labels.platform}：${diagnostics.platform}`,
    `${content.labels.connectivity}：${diagnostics.online ? content.online : content.offline}`,
    `${content.labels.storage}：${diagnostics.storage.ready ? content.ready : content.notReady}`,
    `${content.labels.schemaVersion}：${countLabel(diagnostics.storage.schemaVersion)}`,
    `${content.labels.cipher}：${diagnostics.storage.cipherAvailable ? content.available : content.unavailable}`,
    `${content.labels.search}：${diagnostics.storage.fts5Available ? content.available : content.unavailable}`,
    `${content.labels.credentialStore}：${credentialStoreLabels[diagnostics.storage.credentialStore]}`,
    `${content.labels.storageStatus}：${storageStatus}`,
    "",
    `${content.labels.providers}：`,
  ];
  for (const provider of diagnostics.providers) {
    lines.push(
      `${providerLabels[provider.provider]}：${provider.configured ? content.configured : content.notConfigured}；${content.labels.accountCount} ${countLabel(provider.accountCount)}；${content.labels.connectedCount} ${countLabel(provider.connectedCount)}；${content.labels.reconnectCount} ${countLabel(provider.reconnectCount)}`,
    );
  }
  return lines.join("\n");
}
