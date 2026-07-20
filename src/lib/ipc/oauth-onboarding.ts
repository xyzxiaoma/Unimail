import {
  cancelOauthOnboarding as cancelOauthOnboardingCommand,
  connectedAccounts as connectedAccountsCommand,
  oauthOnboardingStatus as oauthOnboardingStatusCommand,
  startOauthOnboarding as startOauthOnboardingCommand,
  type ConnectedAccountSummary,
  type OAuthOnboardingCommandError,
  type OAuthOnboardingErrorCode,
  type OAuthOnboardingState,
  type OAuthOnboardingStatus,
  type Provider,
} from "./bindings";
import { isRecord } from "./decode";

export type {
  ConnectedAccountSummary,
  OAuthOnboardingCommandError,
  OAuthOnboardingState,
  OAuthOnboardingStatus,
  Provider,
} from "./bindings";

export type OAuthProvider = Extract<Provider, "gmail" | "outlook">;

const onboardingStates = new Set<OAuthOnboardingState>([
  "unconfigured",
  "idle",
  "waiting_for_browser",
  "exchanging",
  "connected",
  "cancelled",
  "failed",
]);

const providers = new Set<Provider>(["gmail", "outlook", "qq", "netease"]);
const oauthProviders = new Set<OAuthProvider>(["gmail", "outlook"]);
const authStates = new Set<ConnectedAccountSummary["authState"]>([
  "connected",
  "needs_authentication",
  "unavailable",
]);

const providerNames: Record<Provider, string> = {
  gmail: "Gmail",
  outlook: "Outlook",
  qq: "邮箱",
  netease: "邮箱",
};

function errorContract(
  provider: Provider,
  code: OAuthOnboardingErrorCode,
): Pick<OAuthOnboardingCommandError, "message" | "retryable"> {
  const name = providerNames[provider];
  const retryable = code !== "not_configured";
  switch (code) {
    case "not_configured":
      return {
        message:
          provider === "gmail" || provider === "outlook"
            ? `当前构建未配置 ${name} 接入。`
            : "当前构建未配置此邮箱接入。",
        retryable,
      };
    case "browser_open_failed":
      return { message: "无法打开系统浏览器，请重试。", retryable };
    case "callback_invalid":
      return { message: `${name} 授权回调无效，请重新连接。`, retryable };
    case "authorization_denied":
      return { message: `你已取消 ${name} 授权。`, retryable };
    case "timed_out":
      return { message: `${name} 授权已超时，请重试。`, retryable };
    case "cancelled":
      return { message: `已取消 ${name} 连接。`, retryable };
    case "authentication_failed":
      return { message: `${name} 授权失败，请重新连接。`, retryable };
    case "provider_unavailable":
      return { message: `暂时无法连接 ${name}，请稍后重试。`, retryable };
    case "storage_unavailable":
      return { message: `无法保存 ${name} 账户，请检查本地加密存储。`, retryable };
    case "internal":
      return { message: `${name} 连接暂时不可用。`, retryable };
  }
}

const errorCodes = new Set<OAuthOnboardingErrorCode>([
  "not_configured",
  "browser_open_failed",
  "callback_invalid",
  "authorization_denied",
  "timed_out",
  "cancelled",
  "authentication_failed",
  "provider_unavailable",
  "storage_unavailable",
  "internal",
]);

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.length > 0;
}

function isProvider(value: unknown): value is Provider {
  return typeof value === "string" && providers.has(value as Provider);
}

function isOAuthProvider(value: unknown): value is OAuthProvider {
  return typeof value === "string" && oauthProviders.has(value as OAuthProvider);
}

function isOnboardingState(value: unknown): value is OAuthOnboardingState {
  return typeof value === "string" && onboardingStates.has(value as OAuthOnboardingState);
}

function isErrorCode(value: unknown): value is OAuthOnboardingErrorCode {
  return typeof value === "string" && errorCodes.has(value as OAuthOnboardingErrorCode);
}

export function decodeConnectedAccount(value: unknown): ConnectedAccountSummary {
  if (!isRecord(value)) throw new TypeError("connected_accounts 账户必须为对象");
  const { id, provider, email, displayName, authState } = value;
  if (
    !isNonEmptyString(id) ||
    !isProvider(provider) ||
    !isNonEmptyString(email) ||
    !isNullableString(displayName) ||
    typeof authState !== "string" ||
    !authStates.has(authState as ConnectedAccountSummary["authState"])
  ) {
    throw new TypeError("connected_accounts 返回了无效账户");
  }
  return {
    id,
    provider,
    email,
    displayName,
    authState: authState as ConnectedAccountSummary["authState"],
  };
}

export function decodeConnectedAccounts(value: unknown): ConnectedAccountSummary[] {
  if (!Array.isArray(value)) throw new TypeError("connected_accounts 必须返回数组");
  return value.map(decodeConnectedAccount);
}

export function decodeOAuthOnboardingCommandError(value: unknown): OAuthOnboardingCommandError {
  if (!isRecord(value)) throw new TypeError("邮箱连接错误必须为对象");
  const { provider, code, message, retryable } = value;
  if (!isProvider(provider) || !isErrorCode(code)) {
    throw new TypeError("邮箱连接返回了无效错误");
  }
  const contract = errorContract(provider, code);
  if (message !== contract.message || retryable !== contract.retryable) {
    throw new TypeError("邮箱连接返回了无效错误");
  }
  return { provider, code, message, retryable };
}

export function decodeOAuthOnboardingStatus(value: unknown): OAuthOnboardingStatus {
  if (!isRecord(value)) throw new TypeError("oauth_onboarding_status 必须返回对象");
  const { provider, state, flowId, account, error } = value;
  if (
    !isOAuthProvider(provider) ||
    !isOnboardingState(state) ||
    !isNullableString(flowId) ||
    flowId === "" ||
    (account !== null && !isRecord(account)) ||
    (error !== null && !isRecord(error))
  ) {
    throw new TypeError("oauth_onboarding_status 返回了无效数据");
  }
  const decodedAccount = account === null ? null : decodeConnectedAccount(account);
  const decodedError = error === null ? null : decodeOAuthOnboardingCommandError(error);
  if (decodedAccount !== null && decodedAccount.provider !== provider) {
    throw new TypeError("oauth_onboarding_status 账户提供商不匹配");
  }
  if (decodedError !== null && decodedError.provider !== provider) {
    throw new TypeError("oauth_onboarding_status 错误提供商不匹配");
  }
  switch (state) {
    case "idle":
      if (flowId !== null || decodedAccount !== null || decodedError !== null)
        throw new TypeError("oauth_onboarding_status idle 状态字段无效");
      break;
    case "unconfigured":
      if (flowId !== null || decodedAccount !== null || decodedError?.code !== "not_configured")
        throw new TypeError("oauth_onboarding_status unconfigured 状态字段无效");
      break;
    case "waiting_for_browser":
    case "exchanging":
      if (!isNonEmptyString(flowId) || decodedAccount !== null || decodedError !== null)
        throw new TypeError("oauth_onboarding_status 活动状态字段无效");
      break;
    case "connected":
      if (flowId !== null || decodedAccount === null || decodedError !== null)
        throw new TypeError("oauth_onboarding_status connected 状态字段无效");
      break;
    case "cancelled":
      if (
        decodedAccount !== null ||
        (decodedError?.code !== "cancelled" && decodedError?.code !== "authorization_denied")
      )
        throw new TypeError("oauth_onboarding_status cancelled 状态字段无效");
      break;
    case "failed":
      if (
        decodedAccount !== null ||
        decodedError === null ||
        decodedError.code === "not_configured" ||
        decodedError.code === "cancelled"
      )
        throw new TypeError("oauth_onboarding_status failed 状态字段无效");
      break;
  }
  return { provider, state, flowId, account: decodedAccount, error: decodedError };
}

export async function getOAuthOnboardingStatus(
  provider: OAuthProvider,
): Promise<OAuthOnboardingStatus> {
  return decodeOAuthOnboardingStatus(await oauthOnboardingStatusCommand(provider));
}

export async function startOAuthOnboarding(
  provider: OAuthProvider,
  accountId: string | null,
): Promise<OAuthOnboardingStatus> {
  return decodeOAuthOnboardingStatus(await startOauthOnboardingCommand(provider, accountId));
}

export async function cancelOAuthOnboarding(
  provider: OAuthProvider,
  flowId: string,
): Promise<OAuthOnboardingStatus> {
  return decodeOAuthOnboardingStatus(await cancelOauthOnboardingCommand(provider, flowId));
}

export async function getConnectedAccounts(): Promise<ConnectedAccountSummary[]> {
  return decodeConnectedAccounts(await connectedAccountsCommand());
}
