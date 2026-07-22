import type { CredentialStoreKind, Provider, StorageErrorCode } from "../lib/ipc/bindings";

export const securityDiagnosticsContent = {
  action: "安全与诊断",
  title: "安全与诊断",
  introduction: "这些信息只在本机生成，不会自动上传，也不包含邮箱地址、邮件内容或本地路径。",
  loading: "正在读取本地安全状态…",
  loadFailed: "暂时无法读取安全诊断，请稍后重试。",
  retry: "重试",
  close: "关闭",
  selectableText: "可选择的诊断文本",
  available: "可用",
  unavailable: "不可用",
  ready: "已就绪",
  notReady: "未就绪",
  online: "在线",
  offline: "离线",
  configured: "已配置",
  notConfigured: "未配置",
  none: "无",
  labels: {
    appVersion: "应用版本",
    platform: "平台",
    connectivity: "联网状态",
    storage: "本地加密存储",
    schemaVersion: "数据库版本",
    cipher: "SQLCipher 加密",
    search: "本地搜索组件",
    credentialStore: "系统凭据保护",
    storageStatus: "存储状态",
    providers: "邮箱服务商",
    accountCount: "账户总数",
    connectedCount: "正常数",
    reconnectCount: "需重连数",
  },
} as const;

export const providerLabels: Record<Provider, string> = {
  gmail: "Gmail",
  outlook: "Outlook",
  qq: "QQ 邮箱",
  netease: "163 邮箱",
};

export const credentialStoreLabels: Record<CredentialStoreKind, string> = {
  windows: "Windows 凭据管理器",
  macos: "macOS 钥匙串",
  unsupported: "当前平台不支持",
};

export const storageStatusLabels: Record<StorageErrorCode, string> = {
  credential_store_unavailable: "系统凭据保护暂不可用",
  database_key_unavailable: "数据库安全密钥不可用",
  database_key_invalid: "数据库安全密钥无效",
  database_open_failed: "本地数据库无法打开",
  cipher_unavailable: "加密数据库组件不可用",
  fts5_unavailable: "本地搜索组件不可用",
  migration_failed: "数据库升级失败",
  storage_busy: "本地存储正忙",
  not_found: "本地数据未找到",
  revision_conflict: "本地数据版本冲突",
  constraint_violation: "本地存储规则冲突",
  invalid_data: "本地存储数据无效",
  cleanup_pending: "本地账户数据仍在清理",
  internal: "本地存储暂不可用",
};
