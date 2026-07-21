import { useId, useRef, useState } from "react";
import { authorizationCodeCopy } from "../../content/authorization-code-onboarding.zh-CN";
import {
  connectAuthorizationCodeAccount,
  decodeAuthorizationCodeError,
  type AuthorizationCodeProvider,
} from "../../lib/ipc/authorization-code-onboarding";
import type { ConnectedAccountSummary } from "../../lib/ipc/oauth-onboarding";

type Props = {
  initialProvider: AuthorizationCodeProvider;
  reconnectAccount: ConnectedAccountSummary | null;
  onClose: () => void;
  onConnected: (account: ConnectedAccountSummary) => void;
};

export function AuthorizationCodeOnboardingDialog({
  initialProvider,
  reconnectAccount,
  onClose,
  onConnected,
}: Props) {
  const titleId = useId();
  const descriptionId = useId();
  const secretRef = useRef<HTMLInputElement>(null);
  const [provider, setProvider] = useState(initialProvider);
  const [accountAddress, setAccountAddress] = useState(reconnectAccount?.email ?? "");
  const [authorizationCode, setAuthorizationCode] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const copy = authorizationCodeCopy[provider];

  const connect = async () => {
    const normalized = accountAddress.trim().toLowerCase();
    if (!normalized.endsWith(`@${copy.domain}`) || normalized.startsWith("@")) {
      setError(`请输入完整的 @${copy.domain} 邮箱地址。`);
      return;
    }
    if (!authorizationCode.trim()) {
      setError("请输入邮箱服务商生成的授权码，不要填写邮箱密码。");
      secretRef.current?.focus();
      return;
    }
    setPending(true);
    setError(null);
    try {
      const account = await connectAuthorizationCodeAccount(
        provider,
        reconnectAccount?.id ?? null,
        normalized,
        authorizationCode,
      );
      setAuthorizationCode("");
      onConnected(account);
      onClose();
    } catch (value: unknown) {
      setAuthorizationCode("");
      try {
        setError(decodeAuthorizationCodeError(value).message);
      } catch {
        setError("暂时无法连接邮箱，请检查网络、邮箱地址和授权码后重试。");
      }
    } finally {
      setPending(false);
    }
  };

  return (
    <div className="oauth-dialog-backdrop">
      <div
        aria-describedby={descriptionId}
        aria-labelledby={titleId}
        aria-modal="true"
        className="oauth-dialog"
        role="dialog"
      >
        <header className="oauth-dialog-header">
          <div>
            <p className="eyebrow">邮箱账户</p>
            <h2 id={titleId}>{copy.title}</h2>
          </div>
          <button
            aria-label="关闭邮箱连接窗口"
            className="icon-button oauth-close-button"
            disabled={pending}
            onClick={onClose}
            type="button"
          >
            <span aria-hidden="true">×</span>
          </button>
        </header>

        <p className="oauth-dialog-introduction" id={descriptionId}>
          {copy.guidance}
        </p>

        {!reconnectAccount && (
          <div aria-label="选择邮箱提供商" className="oauth-provider-choice" role="group">
            {(["qq", "netease"] as const).map((candidate) => (
              <button
                aria-pressed={provider === candidate}
                disabled={pending}
                key={candidate}
                onClick={() => {
                  setProvider(candidate);
                  setAccountAddress("");
                  setAuthorizationCode("");
                  setError(null);
                }}
                type="button"
              >
                {authorizationCodeCopy[candidate].providerName}
              </button>
            ))}
          </div>
        )}

        <div className="authorization-code-fields">
          <label>
            邮箱地址
            <input
              autoComplete="email"
              disabled={pending || reconnectAccount !== null}
              onChange={(event) => setAccountAddress(event.target.value)}
              placeholder={copy.addressPlaceholder}
              type="email"
              value={accountAddress}
            />
          </label>
          <label>
            IMAP/SMTP 授权码
            <input
              autoComplete="off"
              disabled={pending}
              onChange={(event) => setAuthorizationCode(event.target.value)}
              ref={secretRef}
              type="password"
              value={authorizationCode}
            />
          </label>
        </div>

        <section aria-live="polite" className="oauth-status-card">
          <span aria-hidden="true" className="oauth-status-mark idle">
            {provider === "qq" ? "Q" : "1"}
          </span>
          <div>
            <h3>{pending ? "正在验证并保存账户" : "授权码仅保存在系统凭据库"}</h3>
            <p>{error ?? "Unimail 不会把授权码写入邮件数据库、日志或错误信息。"}</p>
          </div>
        </section>

        <footer className="oauth-dialog-actions">
          <button disabled={pending} onClick={onClose} type="button">
            取消
          </button>
          <button
            className="oauth-primary-action"
            disabled={pending}
            onClick={() => void connect()}
            type="button"
          >
            {pending ? "正在连接…" : reconnectAccount ? "重新连接" : "连接邮箱"}
          </button>
        </footer>
      </div>
    </div>
  );
}
