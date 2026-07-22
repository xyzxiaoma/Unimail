import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import "./App.css";
import { OAuthOnboardingDialog } from "./features/accounts/OAuthOnboardingDialog";
import { AuthorizationCodeOnboardingDialog } from "./features/accounts/AuthorizationCodeOnboardingDialog";
import { ComposePanel } from "./features/compose/ComposePanel";
import { DraftsView } from "./features/compose/DraftsView";
import { SentView } from "./features/compose/SentView";
import { SecurityDiagnosticsDialog } from "./features/security/SecurityDiagnosticsDialog";
import { securityDiagnosticsContent } from "./content/security-diagnostics.zh-CN";
import type { AuthorizationCodeProvider } from "./lib/ipc/authorization-code-onboarding";
import { MailWorkspace } from "./features/inbox/MailWorkspace";
import { getApplicationInfo, type ApplicationInfo } from "./lib/ipc/application-info";
import { createLocalReplyDraft, reportDesktopConnectivity } from "./lib/ipc/compose";
import { getConnectedAccounts, type ConnectedAccountSummary } from "./lib/ipc/oauth-onboarding";
import {
  decodeStorageCommandError,
  getStorageStatus,
  type StorageStatus,
} from "./lib/ipc/storage-status";

type IconName =
  | "chevron"
  | "compose"
  | "draft"
  | "inbox"
  | "mail"
  | "more"
  | "paper-plane"
  | "search"
  | "settings"
  | "sparkles"
  | "sync"
  | "x";

const iconPaths: Record<IconName, ReactNode> = {
  chevron: <path d="m9 18 6-6-6-6" />,
  compose: (
    <>
      <path d="M12 20h9" />
      <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L8 18l-4 1 1-4Z" />
    </>
  ),
  draft: (
    <>
      <path d="M14 2H6a2 2 0 0 0-2 2v16h14V6Z" />
      <path d="M14 2v5h5M8 12h6M8 16h5" />
    </>
  ),
  inbox: (
    <>
      <path d="m4 5-2 9v5h20v-5l-2-9Z" />
      <path d="M2 14h5l2 3h6l2-3h5" />
    </>
  ),
  mail: (
    <>
      <rect x="3" y="5" width="18" height="14" rx="2" />
      <path d="m3 7 9 6 9-6" />
    </>
  ),
  more: (
    <>
      <circle cx="5" cy="12" r="1" fill="currentColor" stroke="none" />
      <circle cx="12" cy="12" r="1" fill="currentColor" stroke="none" />
      <circle cx="19" cy="12" r="1" fill="currentColor" stroke="none" />
    </>
  ),
  "paper-plane": (
    <>
      <path d="m22 2-7 20-4-9-9-4Z" />
      <path d="M22 2 11 13" />
    </>
  ),
  search: (
    <>
      <circle cx="11" cy="11" r="7" />
      <path d="m20 20-4-4" />
    </>
  ),
  settings: (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-2.8 2.8-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6v.2h-4V21a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1L4.2 17l.1-.1a1.7 1.7 0 0 0 .3-1.9A1.7 1.7 0 0 0 3 14H2.8v-4H3a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9L4.2 7 7 4.2l.1.1A1.7 1.7 0 0 0 9 4.6 1.7 1.7 0 0 0 10 3v-.2h4V3a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1L19.8 7l-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.2v4H21a1.7 1.7 0 0 0-1.6 1Z" />
    </>
  ),
  sparkles: (
    <>
      <path d="m12 3 1.2 3.2L16 7.5l-2.8 1.3L12 12l-1.2-3.2L8 7.5l2.8-1.3ZM5 14l.8 2.2L8 17l-2.2.8L5 20l-.8-2.2L2 17l2.2-.8ZM19 13l.7 1.8 1.8.7-1.8.7L19 18l-.7-1.8-1.8-.7 1.8-.7Z" />
    </>
  ),
  sync: (
    <>
      <path d="M20 7h-5V2" />
      <path d="M20 7a8 8 0 0 0-14-2M4 17h5v5" />
      <path d="M4 17a8 8 0 0 0 14 2" />
    </>
  ),
  x: <path d="m6 6 12 12M18 6 6 18" />,
};

function Icon({ name, size = 18 }: { name: IconName; size?: number }) {
  return (
    <svg
      aria-hidden="true"
      className="icon"
      fill="none"
      height={size}
      viewBox="0 0 24 24"
      width={size}
    >
      {iconPaths[name]}
    </svg>
  );
}

type MailView = "inbox" | "sent" | "drafts";

const folders: Array<{ id: MailView; icon: IconName; label: string; count?: number }> = [
  { id: "inbox", icon: "inbox", label: "收件箱", count: 0 },
  { id: "sent", icon: "paper-plane", label: "已发送" },
  { id: "drafts", icon: "draft", label: "草稿" },
];

function Sidebar({
  accounts,
  activeView,
  onAddAccount,
  onCompose,
  onDiagnostics,
  onViewChange,
}: {
  accounts: ConnectedAccountSummary[];
  activeView: MailView;
  onAddAccount: (account: ConnectedAccountSummary | null, opener: HTMLButtonElement) => void;
  onCompose: () => void;
  onDiagnostics: (opener: HTMLButtonElement) => void;
  onViewChange: (view: MailView) => void;
}) {
  return (
    <aside className="sidebar" aria-label="邮箱导航">
      <div className="brand" aria-label="Unimail">
        <span className="brand-mark">
          <Icon name="mail" size={19} />
        </span>
        <span>Unimail</span>
      </div>

      <button className="compose-button" type="button" onClick={onCompose}>
        <Icon name="compose" />
        写邮件
        <kbd>N</kbd>
      </button>

      <nav aria-label="邮件文件夹">
        <p className="nav-caption">邮箱</p>
        <ul className="nav-list">
          {folders.map((folder) => (
            <li key={folder.label}>
              <button
                aria-current={activeView === folder.id ? "page" : undefined}
                className={activeView === folder.id ? "nav-item active" : "nav-item"}
                onClick={() => onViewChange(folder.id)}
                type="button"
              >
                <Icon name={folder.icon} />
                <span>{folder.label}</span>
                {folder.count !== undefined && <span className="nav-count">{folder.count}</span>}
              </button>
            </li>
          ))}
        </ul>
      </nav>

      <div className="sidebar-spacer" />
      <section className="account-card" aria-labelledby="account-heading">
        <div className="account-card-heading">
          <span className="account-icon">
            <Icon name="mail" size={16} />
          </span>
          <div>
            <h2 id="account-heading">{accounts.length > 0 ? "邮箱账户" : "添加邮箱账户"}</h2>
            <p>
              {accounts.length > 0
                ? `已连接 ${String(accounts.length)} 个账户`
                : "集中管理你的所有邮件"}
            </p>
          </div>
        </div>
        {accounts.length > 0 && (
          <ul className="account-list">
            {accounts.map((account) => {
              const providerName =
                account.provider === "outlook"
                  ? "Outlook"
                  : account.provider === "qq"
                    ? "QQ 邮箱"
                    : account.provider === "netease"
                      ? "163 邮箱"
                      : "Gmail";
              const reconnectLabel = `重新连接 ${providerName}`;
              return (
                <li key={account.id}>
                  <button
                    type="button"
                    onClick={(event) => onAddAccount(account, event.currentTarget)}
                  >
                    <span>{account.displayName ?? providerName}</span>
                    <small>{account.email}</small>
                    {account.authState === "needs_authentication" && <em>{reconnectLabel}</em>}
                  </button>
                </li>
              );
            })}
          </ul>
        )}
        <button type="button" onClick={(event) => onAddAccount(null, event.currentTarget)}>
          {accounts.length > 0 ? "添加另一个账户" : "开始设置"}
        </button>
      </section>
      <button
        className="settings-button"
        type="button"
        onClick={(event) => onDiagnostics(event.currentTarget)}
      >
        <Icon name="settings" />
        {securityDiagnosticsContent.action}
      </button>
    </aside>
  );
}

function StatusBar({
  appInfo,
  storageMessage,
  storageReady,
  syncMessage,
}: {
  appInfo: ApplicationInfo | null;
  storageMessage: string;
  storageReady: boolean | null;
  syncMessage: string;
}) {
  return (
    <footer className="status-bar">
      <div className="status-group" aria-live="polite">
        <span
          aria-hidden="true"
          className={`status-dot${storageReady === false ? " warning" : storageReady === null ? " neutral" : ""}`}
        />
        <span>{appInfo ? `${appInfo.name} ${appInfo.version}` : "本地模式"}</span>
        <span className="status-divider" />
        <span className="offline-badge">{storageMessage}</span>
      </div>
      <div className="status-group" aria-live="polite">
        <span>{syncMessage}</span>
        <span className="status-divider" />
        <span>{appInfo?.platform ?? "桌面端"}</span>
      </div>
    </footer>
  );
}

export default function App() {
  const [composeOpen, setComposeOpen] = useState(false);
  const [composeDraftId, setComposeDraftId] = useState<string | null>(null);
  const [activeView, setActiveView] = useState<MailView>("inbox");
  const [appInfo, setAppInfo] = useState<ApplicationInfo | null>(null);
  const [storageStatus, setStorageStatus] = useState<StorageStatus | null>(null);
  const [storageMessage, setStorageMessage] = useState("正在检查加密存储");
  const [syncMessage, setSyncMessage] = useState("等待添加账户");
  const [connectedAccounts, setConnectedAccounts] = useState<ConnectedAccountSummary[]>([]);
  const [oauthDialogOpen, setOAuthDialogOpen] = useState(false);
  const [authorizationCodeProvider, setAuthorizationCodeProvider] =
    useState<AuthorizationCodeProvider | null>(null);
  const [reconnectAccount, setReconnectAccount] = useState<ConnectedAccountSummary | null>(null);
  const [securityDialogOpen, setSecurityDialogOpen] = useState(false);
  const composeButtonRef = useRef<HTMLDivElement>(null);
  const oauthDialogOpenerRef = useRef<HTMLButtonElement | null>(null);
  const securityDialogOpenerRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    let active = true;
    getApplicationInfo()
      .then((info) => {
        if (active) setAppInfo(info);
      })
      .catch(() => {
        /* Web preview and unavailable desktop IPC remain usable. */
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    void getConnectedAccounts()
      .then((accounts) => {
        if (!active) return;
        setConnectedAccounts(accounts);
        const needsAuthentication = accounts.find(
          (account) => account.authState === "needs_authentication",
        );
        if (needsAuthentication) {
          const name =
            needsAuthentication.provider === "outlook"
              ? "Outlook"
              : needsAuthentication.provider === "qq"
                ? "QQ 邮箱"
                : needsAuthentication.provider === "netease"
                  ? "163 邮箱"
                  : "Gmail";
          setSyncMessage(`${name} 账户需要重新连接`);
        } else if (accounts.length > 0) {
          setSyncMessage(`已连接 ${String(accounts.length)} 个邮箱账户`);
        }
      })
      .catch(() => {
        /* Browser preview remains usable without desktop account IPC. */
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    getStorageStatus()
      .then((status) => {
        if (!active) return;
        setStorageStatus(status);
        setStorageMessage(
          status.ready
            ? `加密存储已就绪 · Schema ${String(status.schemaVersion)}`
            : "加密存储未就绪",
        );
      })
      .catch((error: unknown) => {
        if (!active) return;
        try {
          setStorageMessage(decodeStorageCommandError(error).message);
        } catch {
          setStorageMessage("无法读取加密存储状态");
        }
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    const report = () => {
      void reportDesktopConnectivity(navigator.onLine).catch(() => {
        /* Browser preview and unavailable desktop IPC remain usable. */
      });
    };
    report();
    window.addEventListener("online", report);
    window.addEventListener("offline", report);
    return () => {
      window.removeEventListener("online", report);
      window.removeEventListener("offline", report);
    };
  }, []);

  useEffect(() => {
    const openCompose = (event: KeyboardEvent) => {
      if (oauthDialogOpen || securityDialogOpen) return;
      if (event.key.toLowerCase() === "n" && !event.metaKey && !event.ctrlKey && !event.altKey) {
        const target = event.target;
        if (
          target instanceof Element &&
          target.matches("input, textarea, [contenteditable='true']")
        ) {
          return;
        }
        event.preventDefault();
        setComposeDraftId(null);
        setComposeOpen(true);
      }
    };
    window.addEventListener("keydown", openCompose);
    return () => window.removeEventListener("keydown", openCompose);
  }, [oauthDialogOpen, securityDialogOpen]);

  const closeCompose = () => {
    setComposeOpen(false);
    setComposeDraftId(null);
    composeButtonRef.current?.querySelector<HTMLButtonElement>("button")?.focus();
  };

  const openNewCompose = () => {
    setComposeDraftId(null);
    setComposeOpen(true);
  };

  const openDraft = (draftId: string) => {
    setComposeDraftId(draftId);
    setComposeOpen(true);
  };

  const replyToMessage = (messageId: string) => {
    void createLocalReplyDraft(messageId)
      .then((draft) => {
        setComposeDraftId(draft.id);
        setComposeOpen(true);
      })
      .catch(() => setSyncMessage("无法创建回复草稿，请稍后重试"));
  };

  const openOAuthDialog = useCallback(
    (account: ConnectedAccountSummary | null, opener: HTMLButtonElement) => {
      oauthDialogOpenerRef.current = opener;
      setReconnectAccount(account);
      if (account?.provider === "qq" || account?.provider === "netease") {
        setAuthorizationCodeProvider(account.provider);
        setOAuthDialogOpen(false);
      } else {
        setAuthorizationCodeProvider(null);
        setOAuthDialogOpen(true);
      }
    },
    [],
  );

  const closeOAuthDialog = useCallback(() => {
    setOAuthDialogOpen(false);
    setAuthorizationCodeProvider(null);
    setReconnectAccount(null);
    window.setTimeout(() => oauthDialogOpenerRef.current?.focus(), 0);
  }, []);

  const recordConnectedAccount = useCallback((account: ConnectedAccountSummary) => {
    setConnectedAccounts((accounts) => [
      account,
      ...accounts.filter((existing) => existing.id !== account.id),
    ]);
    const name =
      account.provider === "outlook"
        ? "Outlook"
        : account.provider === "qq"
          ? "QQ 邮箱"
          : account.provider === "netease"
            ? "163 邮箱"
            : "Gmail";
    setSyncMessage(`${name} 已连接，正在准备同步收件箱`);
  }, []);

  const openSecurityDialog = useCallback((opener: HTMLButtonElement) => {
    securityDialogOpenerRef.current = opener;
    setSecurityDialogOpen(true);
  }, []);

  const closeSecurityDialog = useCallback(() => {
    setSecurityDialogOpen(false);
    window.setTimeout(() => securityDialogOpenerRef.current?.focus(), 0);
  }, []);

  return (
    <div className="app-frame">
      <div className="app-content" ref={composeButtonRef}>
        <Sidebar
          accounts={connectedAccounts}
          activeView={activeView}
          onAddAccount={openOAuthDialog}
          onCompose={openNewCompose}
          onDiagnostics={openSecurityDialog}
          onViewChange={setActiveView}
        />
        {activeView === "inbox" ? (
          <MailWorkspace
            accounts={connectedAccounts}
            onAddAccount={(opener) => openOAuthDialog(null, opener)}
            onReply={replyToMessage}
            onSync={() =>
              setSyncMessage(connectedAccounts.length > 0 ? "正在请求邮箱同步" : "尚无可同步账户")
            }
          />
        ) : activeView === "drafts" ? (
          <DraftsView accounts={connectedAccounts} onOpenDraft={openDraft} />
        ) : (
          <SentView accounts={connectedAccounts} onOpenDraft={openDraft} />
        )}
        {composeOpen && (
          <ComposePanel
            key={composeDraftId ?? "new-message"}
            accounts={connectedAccounts}
            draftId={composeDraftId}
            onClose={closeCompose}
            onSent={() => setActiveView("sent")}
          />
        )}
        {oauthDialogOpen && (
          <OAuthOnboardingDialog
            reconnectAccount={reconnectAccount}
            onClose={closeOAuthDialog}
            onConnected={recordConnectedAccount}
            onAuthorizationCodeProvider={(provider) => {
              setOAuthDialogOpen(false);
              setAuthorizationCodeProvider(provider);
            }}
          />
        )}
        {authorizationCodeProvider && (
          <AuthorizationCodeOnboardingDialog
            initialProvider={authorizationCodeProvider}
            reconnectAccount={reconnectAccount}
            onClose={closeOAuthDialog}
            onConnected={recordConnectedAccount}
          />
        )}
        {securityDialogOpen && <SecurityDiagnosticsDialog onClose={closeSecurityDialog} />}
      </div>
      <StatusBar
        appInfo={appInfo}
        storageMessage={storageMessage}
        storageReady={storageStatus?.ready ?? null}
        syncMessage={syncMessage}
      />
    </div>
  );
}
