import {
  cancelGmailOnboarding as cancelGmailOnboardingCommand,
  connectedAccounts as connectedAccountsCommand,
  gmailOnboardingStatus as gmailOnboardingStatusCommand,
  startGmailOnboarding as startGmailOnboardingCommand,
  type ConnectedAccountSummary,
  type GmailOnboardingCommandError,
  type GmailOnboardingErrorCode,
  type GmailOnboardingState,
  type GmailOnboardingStatus,
} from "./bindings";
import { isRecord } from "./decode";

export type {
  ConnectedAccountSummary,
  GmailOnboardingCommandError,
  GmailOnboardingState,
  GmailOnboardingStatus,
} from "./bindings";

const onboardingStates = new Set<GmailOnboardingState>([
  "unconfigured",
  "idle",
  "waiting_for_browser",
  "exchanging",
  "connected",
  "cancelled",
  "failed",
]);

const providers = new Set<ConnectedAccountSummary["provider"]>([
  "gmail",
  "outlook",
  "qq",
  "netease",
]);

const authStates = new Set<ConnectedAccountSummary["authState"]>([
  "connected",
  "needs_authentication",
  "unavailable",
]);

const gmailErrorContracts: Record<
  GmailOnboardingErrorCode,
  Pick<GmailOnboardingCommandError, "message" | "retryable">
> = {
  not_configured: { message: "当前构建未配置 Gmail 接入。", retryable: false },
  browser_open_failed: { message: "无法打开系统浏览器，请重试。", retryable: true },
  callback_invalid: { message: "Gmail 授权回调无效，请重新连接。", retryable: true },
  authorization_denied: { message: "你已取消 Gmail 授权。", retryable: true },
  timed_out: { message: "Gmail 授权已超时，请重试。", retryable: true },
  cancelled: { message: "已取消 Gmail 连接。", retryable: true },
  authentication_failed: { message: "Gmail 授权失败，请重新连接。", retryable: true },
  provider_unavailable: { message: "暂时无法连接 Gmail，请稍后重试。", retryable: true },
  storage_unavailable: {
    message: "无法保存 Gmail 账户，请检查本地加密存储。",
    retryable: true,
  },
  internal: { message: "Gmail 连接暂时不可用。", retryable: true },
};

const gmailErrorCodes = new Set<GmailOnboardingErrorCode>(
  Object.keys(gmailErrorContracts) as GmailOnboardingErrorCode[],
);

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.length > 0;
}

function isOnboardingState(value: unknown): value is GmailOnboardingState {
  return typeof value === "string" && onboardingStates.has(value as GmailOnboardingState);
}

function isGmailErrorCode(value: unknown): value is GmailOnboardingErrorCode {
  return typeof value === "string" && gmailErrorCodes.has(value as GmailOnboardingErrorCode);
}

export function decodeConnectedAccount(value: unknown): ConnectedAccountSummary {
  if (!isRecord(value)) {
    throw new TypeError("connected_accounts 账户必须为对象");
  }

  const { id, provider, email, displayName, authState } = value;
  if (
    typeof id !== "string" ||
    id.length === 0 ||
    typeof provider !== "string" ||
    !providers.has(provider as ConnectedAccountSummary["provider"]) ||
    typeof email !== "string" ||
    email.length === 0 ||
    !isNullableString(displayName) ||
    typeof authState !== "string" ||
    !authStates.has(authState as ConnectedAccountSummary["authState"])
  ) {
    throw new TypeError("connected_accounts 返回了无效账户");
  }

  return {
    id,
    provider: provider as ConnectedAccountSummary["provider"],
    email,
    displayName,
    authState: authState as ConnectedAccountSummary["authState"],
  };
}

export function decodeConnectedAccounts(value: unknown): ConnectedAccountSummary[] {
  if (!Array.isArray(value)) {
    throw new TypeError("connected_accounts 必须返回数组");
  }
  return value.map(decodeConnectedAccount);
}

export function decodeGmailOnboardingCommandError(value: unknown): GmailOnboardingCommandError {
  if (!isRecord(value)) {
    throw new TypeError("Gmail 连接错误必须为对象");
  }

  const { code, message, retryable } = value;
  if (!isGmailErrorCode(code)) {
    throw new TypeError("Gmail 连接返回了无效错误");
  }
  const contract = gmailErrorContracts[code];
  if (message !== contract.message || retryable !== contract.retryable) {
    throw new TypeError("Gmail 连接返回了无效错误");
  }
  return { code, message, retryable };
}

export function decodeGmailOnboardingStatus(value: unknown): GmailOnboardingStatus {
  if (!isRecord(value)) {
    throw new TypeError("gmail_onboarding_status 必须返回对象");
  }

  const { state, flowId, account, error } = value;
  if (
    !isOnboardingState(state) ||
    !isNullableString(flowId) ||
    flowId === "" ||
    (account !== null && !isRecord(account)) ||
    (error !== null && !isRecord(error))
  ) {
    throw new TypeError("gmail_onboarding_status 返回了无效数据");
  }

  const decodedAccount = account === null ? null : decodeConnectedAccount(account);
  const decodedError = error === null ? null : decodeGmailOnboardingCommandError(error);
  switch (state) {
    case "idle":
      if (flowId !== null || decodedAccount !== null || decodedError !== null) {
        throw new TypeError("gmail_onboarding_status idle 状态字段无效");
      }
      break;
    case "unconfigured":
      if (flowId !== null || decodedAccount !== null || decodedError?.code !== "not_configured") {
        throw new TypeError("gmail_onboarding_status unconfigured 状态字段无效");
      }
      break;
    case "waiting_for_browser":
    case "exchanging":
      if (!isNonEmptyString(flowId) || decodedAccount !== null || decodedError !== null) {
        throw new TypeError("gmail_onboarding_status 活动状态字段无效");
      }
      break;
    case "connected":
      if (flowId !== null || decodedAccount?.provider !== "gmail" || decodedError !== null) {
        throw new TypeError("gmail_onboarding_status connected 状态字段无效");
      }
      break;
    case "cancelled":
      if (
        decodedAccount !== null ||
        (decodedError?.code !== "cancelled" && decodedError?.code !== "authorization_denied")
      ) {
        throw new TypeError("gmail_onboarding_status cancelled 状态字段无效");
      }
      break;
    case "failed":
      if (
        decodedAccount !== null ||
        decodedError === null ||
        decodedError.code === "not_configured" ||
        decodedError.code === "cancelled"
      ) {
        throw new TypeError("gmail_onboarding_status failed 状态字段无效");
      }
      break;
  }

  return { state, flowId, account: decodedAccount, error: decodedError };
}

export async function getGmailOnboardingStatus(): Promise<GmailOnboardingStatus> {
  return decodeGmailOnboardingStatus(await gmailOnboardingStatusCommand());
}

export async function startGmailOnboarding(
  accountId: string | null,
): Promise<GmailOnboardingStatus> {
  return decodeGmailOnboardingStatus(await startGmailOnboardingCommand(accountId));
}

export async function cancelGmailOnboarding(flowId: string): Promise<GmailOnboardingStatus> {
  return decodeGmailOnboardingStatus(await cancelGmailOnboardingCommand(flowId));
}

export async function getConnectedAccounts(): Promise<ConnectedAccountSummary[]> {
  return decodeConnectedAccounts(await connectedAccountsCommand());
}
