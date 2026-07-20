import { useCallback, useEffect, useId, useRef, useState } from "react";
import { gmailOnboardingCopy as copy } from "../../content/gmail-onboarding.zh-CN";
import {
  cancelGmailOnboarding,
  decodeGmailOnboardingCommandError,
  getGmailOnboardingStatus,
  startGmailOnboarding,
  type ConnectedAccountSummary,
  type GmailOnboardingStatus,
} from "../../lib/ipc/gmail-onboarding";

const pollIntervalMs = 750;

type GmailOnboardingDialogProps = {
  reconnectAccount: ConnectedAccountSummary | null;
  onClose: () => void;
  onConnected: (account: ConnectedAccountSummary) => void;
};

function isActive(status: GmailOnboardingStatus | null): boolean {
  return status?.state === "waiting_for_browser" || status?.state === "exchanging";
}

function safeErrorMessage(error: unknown): string {
  try {
    return decodeGmailOnboardingCommandError(error).message;
  } catch {
    return copy.genericUnavailable;
  }
}

function accountStateLabel(account: ConnectedAccountSummary): string {
  switch (account.authState) {
    case "connected":
      return copy.connected;
    case "needs_authentication":
      return copy.needsAuthentication;
    case "unavailable":
      return copy.unavailable;
  }
  throw new TypeError("未知 Gmail 账户状态");
}

function statusContent(status: GmailOnboardingStatus | null): {
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
  throw new TypeError("未知 Gmail 连接状态");
}

export function GmailOnboardingDialog({
  reconnectAccount,
  onClose,
  onConnected,
}: GmailOnboardingDialogProps) {
  const titleId = useId();
  const descriptionId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const mountedRef = useRef(true);
  const commandPendingRef = useRef(false);
  const [status, setStatus] = useState<GmailOnboardingStatus | null>(null);
  const [commandPending, setCommandPending] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);

  const acceptStatus = useCallback(
    (nextStatus: GmailOnboardingStatus) => {
      if (!mountedRef.current) return;
      setStatus(nextStatus);
      setLocalError(null);
      if (nextStatus.state === "connected" && nextStatus.account) {
        onConnected(nextStatus.account);
      }
    },
    [onConnected],
  );

  useEffect(() => {
    mountedRef.current = true;
    titleRef.current?.focus();
    void getGmailOnboardingStatus()
      .then(acceptStatus)
      .catch((error: unknown) => {
        if (mountedRef.current) setLocalError(safeErrorMessage(error));
      });
    return () => {
      mountedRef.current = false;
    };
  }, [acceptStatus]);

  useEffect(() => {
    if (!isActive(status)) return;
    let active = true;
    let timer = 0;
    const poll = () => {
      void getGmailOnboardingStatus()
        .then((nextStatus) => {
          if (active) acceptStatus(nextStatus);
        })
        .catch((error: unknown) => {
          if (active && mountedRef.current) setLocalError(safeErrorMessage(error));
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
  }, [acceptStatus, status]);

  const close = useCallback(async () => {
    if (commandPendingRef.current) return;
    commandPendingRef.current = true;
    const flowId = isActive(status) ? status?.flowId : null;
    if (flowId) {
      setCommandPending(true);
      try {
        await cancelGmailOnboarding(flowId);
      } catch {
        // Closing must remain available even if desktop IPC is no longer reachable.
      }
    }
    if (mountedRef.current) onClose();
  }, [onClose, status]);

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
      acceptStatus(await startGmailOnboarding(reconnectAccount?.id ?? null));
    } catch (error: unknown) {
      if (mountedRef.current) setLocalError(safeErrorMessage(error));
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
      acceptStatus(await cancelGmailOnboarding(status.flowId));
    } catch (error: unknown) {
      if (mountedRef.current) setLocalError(safeErrorMessage(error));
    } finally {
      commandPendingRef.current = false;
      if (mountedRef.current) setCommandPending(false);
    }
  };

  const content = statusContent(status);
  const connectedAccount = status?.account ?? reconnectAccount;
  const canStart =
    status?.state === "idle" ||
    status?.state === "connected" ||
    status?.state === "cancelled" ||
    status?.state === "failed";

  return (
    <div className="gmail-dialog-backdrop">
      <div
        aria-describedby={descriptionId}
        aria-labelledby={titleId}
        aria-modal="true"
        className="gmail-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="gmail-dialog-header">
          <div>
            <p className="eyebrow">{copy.eyebrow}</p>
            <h2 id={titleId} ref={titleRef} tabIndex={-1}>
              {copy.title}
            </h2>
          </div>
          <button
            aria-label={copy.close}
            className="icon-button gmail-close-button"
            disabled={commandPending}
            onClick={() => void close()}
            type="button"
          >
            <span aria-hidden="true">×</span>
          </button>
        </header>

        <p className="gmail-dialog-introduction" id={descriptionId}>
          {copy.introduction}
        </p>

        <section aria-live="polite" aria-busy={commandPending} className="gmail-status-card">
          <span aria-hidden="true" className={`gmail-status-mark ${status?.state ?? "loading"}`}>
            G
          </span>
          <div>
            <h3>{content.title}</h3>
            <p>{localError ?? content.body}</p>
          </div>
        </section>

        {connectedAccount && (
          <section className="gmail-account-summary" aria-labelledby={`${titleId}-account`}>
            <div>
              <h3 id={`${titleId}-account`}>{copy.connectedAccountsHeading}</h3>
              <strong>{connectedAccount.displayName ?? connectedAccount.email}</strong>
              <span>{connectedAccount.email}</span>
            </div>
            <span className="gmail-account-state">{accountStateLabel(connectedAccount)}</span>
          </section>
        )}

        <p className="gmail-privacy-note">{copy.privacyNote}</p>

        <footer className="gmail-dialog-actions">
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
              className="gmail-primary-action"
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
