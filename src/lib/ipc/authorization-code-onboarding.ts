import { connectAuthorizationCodeAccount as connectCommand } from "./bindings";
import {
  decodeConnectedAccount,
  decodeOAuthOnboardingCommandError,
  type ConnectedAccountSummary,
  type OAuthOnboardingCommandError,
  type Provider,
} from "./oauth-onboarding";

export type AuthorizationCodeProvider = Extract<Provider, "qq" | "netease">;

export async function connectAuthorizationCodeAccount(
  provider: AuthorizationCodeProvider,
  accountId: string | null,
  accountAddress: string,
  authorizationCode: string,
): Promise<ConnectedAccountSummary> {
  return decodeConnectedAccount(
    await connectCommand(provider, accountId, accountAddress, authorizationCode),
  );
}

export function decodeAuthorizationCodeError(value: unknown): OAuthOnboardingCommandError {
  return decodeOAuthOnboardingCommandError(value);
}
