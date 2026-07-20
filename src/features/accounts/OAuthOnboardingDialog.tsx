import { useCallback, useEffect, useId, useRef, useState } from "react";
import { onboardingCopy } from "../../content/oauth-onboarding.zh-CN";
import {
  cancelOAuthOnboarding,
  decodeOAuthOnboardingCommandError,
  getOAuthOnboardingStatus,
  startOAuthOnboarding,
  type ConnectedAccountSummary,
  type OAuthProvider,
  type OAuthOnboardingStatus,
} from "../../lib/ipc/oauth-onboarding";

const pollIntervalMs = 750;

type OAuthOnboardingDialogProps = {
  initialProvider?: OAuthProvider;
  reconnectAccount: ConnectedAccountSummary | null;
  onClose: () => void;
  onConnected: (account: ConnectedAccountSummary) => void;
};

function isActive(status: OAuthOnboardingStatus | null): boolean {
  return status?.state === "waiting_for_browser" || status?.state === "exchanging";
}

type ProviderCopy = ReturnType<typeof onboardingCopy>;

function safeErrorMessage(error: unknown, copy: ProviderCopy): string {
  try {
    return decodeOAuthOnboardingCommandError(error).message;
  } catch {
    return copy.genericUnavailable;
  }
}

function accountStateLabel(account: ConnectedAccountSummary, copy: ProviderCopy): string {
  switch (account.authState) {
    case "connected":
      return copy.connected;
    case "needs_authentication":
      return copy.needsAuthentication;
    case "unavailable":
      return copy.unavailable;
  }
  throw new TypeError("未知 邮箱账户状态");
}

function statusContent(
  status: OAuthOnboardingStatus | null,
  copy: ProviderCopy,
): {
  title: string;
  body: string;
} {
  switch (status?.state) {
    case undefined:
      return { title: copy.refreshing, body: copy.privacyNote };
    case "unconfigured":
      return { title: copy.unconfiguredTitle, body: copy.unconfiguredBody };
    case "idle":
      return { title: copy.configuredTitle, body: copy.configuredBody };
    case "waiting_for_browser":
      return { title: copy.waitingTitle, body: copy.waitingBody };
    case "exchanging":
      return { title: copy.exchangingTitle, body: copy.exchangingBody };
    case "connected":
      return { title: copy.connectedTitle, body: copy.connectedBody };
    case "cancelled":
      return { title: copy.cancelledTitle, body: copy.cancelledBody };
    case "failed":
      return {
        title: copy.failedTitle,
        body: status.error?.message ?? copy.genericUnavailable,
      };
  }
  throw new TypeError("未知 邮箱连接状态");
}

export function OAuthOnboardingDialog({
  initialProvider = "gmail",
  reconnectAccount,
  onClose,
  onConnected,
}: OAuthOnboardingDialogProps) {
  const titleId = useId();
  const descriptionId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const mountedRef = useRef(true);
  const commandPendingRef = useRef(false);
  const reconnectProvider =
    reconnectAccount?.provider === "gmail" || reconnectAccount?.provider === "outlook"
      ? reconnectAccount.provider
      : null;
  const [provider, setProvider] = useState<OAuthProvider>(reconnectProvider ?? initialProvider);
  const [status, setStatus] = useState<OAuthOnboardingStatus | null>(null);
  const [commandPending, setCommandPending] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  const copy = onboardingCopy(provider);

  const acceptStatus = useCallback(
    (nextStatus: OAuthOnboardingStatus) => {
      if (!mountedRef.current || nextStatus.provider !== provider) return;
      setStatus(nextStatus);
      setLocalError(null);
      if (nextStatus.state === "connected" && nextStatus.account) {
        onConnected(nextStatus.account);
      }
    },
    [onConnected, provider],
  );

  useEffect(() => {
    mountedRef.current = true;
    titleRef.current?.focus();
    void getOAuthOnboardingStatus(provider)
      .then(acceptStatus)
      .catch((error: unknown) => {
        if (mountedRef.current) setLocalError(safeErrorMessage(error, copy));
      });
    return () => {
      mountedRef.current = false;
    };
  }, [acceptStatus, copy, provider]);

  useEffect(() => {
    if (!isActive(status)) return;
    let active = true;
    let timer = 0;
    const poll = () => {
      void getOAuthOnboardingStatus(provider)
        .then((nextStatus) => {
          if (active) acceptStatus(nextStatus);
        })
        .catch((error: unknown) => {
          if (active && mountedRef.current) setLocalError(safeErrorMessage(error, copy));
        })
        .finally(() => {
          if (active) timer = window.setTimeout(poll, pollIntervalMs);
        });
    };
    timer = window.setTimeout(poll, pollIntervalMs);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [acceptStatus, copy, provider, status]);

  const close = useCallback(async () => {
    if (commandPendingRef.current) return;
    commandPendingRef.current = true;
    const flowId = isActive(status) ? status?.flowId : null;
    if (flowId) {
      setCommandPending(true);
      try {
        await cancelOAuthOnboarding(provider, flowId);
      } catch {
        // Closing must remain available even if desktop IPC is no longer reachable.
      }
    }
    if (mountedRef.current) onClose();
  }, [onClose, provider, status]);

  useEffect(() => {
    const handleDialogKeys = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        if (!commandPendingRef.current) void close();
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = dialogRef.current?.querySelectorAll<HTMLElement>(
        "button:not(:disabled), [href], input:not(:disabled), [tabindex]:not([tabindex='-1'])",
      );
      if (!focusable?.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const activeElement = document.activeElement;
      const activeIsFocusable = Array.from(focusable).some((element) => element === activeElement);
      if (event.shiftKey && (!activeIsFocusable || activeElement === first)) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && (!activeIsFocusable || activeElement === last)) {
        event.preventDefault();
        first?.focus();
      }
    };
    window.addEventListener("keydown", handleDialogKeys);
    return () => window.removeEventListener("keydown", handleDialogKeys);
  }, [close]);

  const start = async () => {
    if (commandPendingRef.current) return;
    commandPendingRef.current = true;
    setCommandPending(true);
    setLocalError(null);
    try {
      acceptStatus(await startOAuthOnboarding(provider, reconnectAccount?.id ?? null));
    } catch (error: unknown) {
      if (mountedRef.current) setLocalError(safeErrorMessage(error, copy));
    } finally {
      commandPendingRef.current = false;
      if (mountedRef.current) setCommandPending(false);
    }
  };

  const cancel = async () => {
    if (!status?.flowId || commandPendingRef.current) return;
    commandPendingRef.current = true;
    setCommandPending(true);
    setLocalError(null);
    try {
      acceptStatus(await cancelOAuthOnboarding(provider, status.flowId));
    } catch (error: unknown) {
      if (mountedRef.current) setLocalError(safeErrorMessage(error, copy));
    } finally {
      commandPendingRef.current = false;
      if (mountedRef.current) setCommandPending(false);
    }
  };

  const content = statusContent(status, copy);
  const connectedAccount =
    status?.account ?? (reconnectAccount?.provider === provider ? reconnectAccount : null);
  const canStart =
    status?.state === "idle" ||
    status?.state === "connected" ||
    status?.state === "cancelled" ||
    status?.state === "failed";

  return (
    <div className="oauth-dialog-backdrop">
      <div
        aria-describedby={descriptionId}
        aria-labelledby={titleId}
        aria-modal="true"
        className="oauth-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="oauth-dialog-header">
          <div>
            <p className="eyebrow">{copy.eyebrow}</p>
            <h2 id={titleId} ref={titleRef} tabIndex={-1}>
              {copy.title}
            </h2>
          </div>
          <button
            aria-label={copy.close}
            className="icon-button oauth-close-button"
            disabled={commandPending}
            onClick={() => void close()}
            type="button"
          >
            <span aria-hidden="true">×</span>
          </button>
        </header>

        <p className="oauth-dialog-introduction" id={descriptionId}>
          {copy.introduction}
        </p>

        {!reconnectAccount && !isActive(status) && (
          <div aria-label="选择邮箱提供商" className="oauth-provider-choice" role="group">
            {(["gmail", "outlook"] as const).map((candidate) => (
              <button
                aria-pressed={provider === candidate}
                disabled={commandPending}
                key={candidate}
                onClick={() => {
                  setLocalError(null);
                  setProvider(candidate);
                }}
                type="button"
              >
                {onboardingCopy(candidate).providerName}
              </button>
            ))}
          </div>
        )}

        <section aria-live="polite" aria-busy={commandPending} className="oauth-status-card">
          <span aria-hidden="true" className={`oauth-status-mark ${status?.state ?? "loading"}`}>
            {copy.mark}
          </span>
          <div>
            <h3>{content.title}</h3>
            <p>{localError ?? content.body}</p>
          </div>
        </section>

        {connectedAccount && (
          <section className="oauth-account-summary" aria-labelledby={`${titleId}-account`}>
            <div>
              <h3 id={`${titleId}-account`}>{copy.connectedAccountsHeading}</h3>
              <strong>{connectedAccount.displayName ?? connectedAccount.email}</strong>
              <span>{connectedAccount.email}</span>
            </div>
            <span className="oauth-account-state">{accountStateLabel(connectedAccount, copy)}</span>
          </section>
        )}

        <p className="oauth-privacy-note">{copy.privacyNote}</p>

        <footer className="oauth-dialog-actions">
          {isActive(status) ? (
            <button disabled={commandPending} onClick={() => void cancel()} type="button">
              {copy.cancel}
            </button>
          ) : (
            <button disabled={commandPending} onClick={() => void close()} type="button">
              {status?.state === "connected" ? copy.done : copy.dismiss}
            </button>
          )}
          {canStart && (
            <button
              className="oauth-primary-action"
              disabled={commandPending}
              onClick={() => void start()}
              type="button"
            >
              {status.state === "idle" || status.state === "connected"
                ? reconnectAccount
                  ? copy.reconnect
                  : copy.connect
                : copy.retry}
            </button>
          )}
        </footer>
      </div>
    </div>
  );
}
