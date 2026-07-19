import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import "./App.css";
import { getApplicationInfo, type ApplicationInfo } from "./lib/ipc/application-info";

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

const folders: Array<{ icon: IconName; label: string; count?: number }> = [
  { icon: "inbox", label: "收件箱", count: 0 },
  { icon: "paper-plane", label: "已发送" },
  { icon: "draft", label: "草稿" },
];

function Sidebar({ onCompose }: { onCompose: () => void }) {
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
          {folders.map((folder, index) => (
            <li key={folder.label}>
              <button
                aria-current={index === 0 ? "page" : undefined}
                className={index === 0 ? "nav-item active" : "nav-item"}
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
            <h2 id="account-heading">添加邮箱账户</h2>
            <p>集中管理你的所有邮件</p>
          </div>
        </div>
        <button type="button">开始设置</button>
      </section>
      <button className="settings-button" type="button">
        <Icon name="settings" />
        设置
      </button>
    </aside>
  );
}

function MessageList({ onSync }: { onSync: () => void }) {
  const searchId = useId();

  return (
    <section className="message-pane" aria-labelledby="inbox-heading">
      <header className="message-header">
        <div>
          <p className="eyebrow">全部账户</p>
          <h1 id="inbox-heading">收件箱</h1>
        </div>
        <div className="header-actions">
          <button className="icon-button" type="button" onClick={onSync} aria-label="同步邮件">
            <Icon name="sync" />
          </button>
          <button className="icon-button" type="button" aria-label="更多收件箱操作">
            <Icon name="more" />
          </button>
        </div>
      </header>

      <form className="search" role="search" onSubmit={(event) => event.preventDefault()}>
        <label className="sr-only" htmlFor={searchId}>
          搜索邮件
        </label>
        <Icon name="search" />
        <input id={searchId} type="search" placeholder="搜索邮件" autoComplete="off" />
        <kbd>⌘ K</kbd>
      </form>

      <div className="filter-row">
        <div role="group" aria-label="邮件筛选">
          <button className="filter active" type="button" aria-pressed="true">
            全部
          </button>
          <button className="filter" type="button" aria-pressed="false">
            未读
          </button>
        </div>
        <button className="sort-button" type="button">
          最新优先 <Icon name="chevron" size={14} />
        </button>
      </div>

      <div className="empty-list">
        <div className="empty-illustration mail-stack" aria-hidden="true">
          <span className="paper paper-back" />
          <span className="paper paper-front">
            <Icon name="mail" size={28} />
          </span>
          <span className="spark spark-one">✦</span>
          <span className="spark spark-two">✧</span>
        </div>
        <h2>收件箱空空如也</h2>
        <p>添加邮箱账户后，新邮件会出现在这里。</p>
        <button className="secondary-action" type="button">
          添加邮箱账户
        </button>
      </div>
    </section>
  );
}

function ReaderPane() {
  return (
    <section className="reader-pane" aria-labelledby="reader-heading">
      <div className="reader-toolbar" aria-hidden="true">
        <span />
        <span />
        <span />
      </div>
      <div className="reader-empty">
        <div className="empty-illustration reader-art" aria-hidden="true">
          <span className="reader-envelope">
            <Icon name="mail" size={34} />
          </span>
          <span className="reader-orbit orbit-one" />
          <span className="reader-orbit orbit-two" />
          <span className="reader-spark">
            <Icon name="sparkles" size={17} />
          </span>
        </div>
        <h2 id="reader-heading">选择一封邮件开始阅读</h2>
        <p>邮件内容将在这里安静地展开。</p>
        <div className="shortcut-hint">
          <kbd>J</kbd>
          <kbd>K</kbd>
          <span>切换邮件</span>
        </div>
      </div>
    </section>
  );
}

function ComposePanel({ onClose }: { onClose: () => void }) {
  const titleId = useId();

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [onClose]);

  return (
    <section className="compose-panel" role="dialog" aria-labelledby={titleId} aria-modal="false">
      <header>
        <div>
          <p className="eyebrow">新邮件</p>
          <h2 id={titleId}>撰写邮件</h2>
        </div>
        <button className="icon-button" type="button" onClick={onClose} aria-label="关闭写邮件窗口">
          <Icon name="x" />
        </button>
      </header>
      <form onSubmit={(event) => event.preventDefault()}>
        <label>
          收件人
          <input autoFocus type="email" placeholder="添加邮箱账户后即可填写" readOnly />
        </label>
        <label>
          主题
          <input type="text" placeholder="邮件主题" readOnly />
        </label>
        <label className="body-field">
          <span className="sr-only">邮件正文</span>
          <textarea placeholder="邮件编辑功能将在后续版本开放" readOnly />
        </label>
        <footer>
          <span>请先添加邮箱账户</span>
          <button type="submit" disabled>
            <Icon name="paper-plane" size={16} /> 发送
          </button>
        </footer>
      </form>
    </section>
  );
}

function StatusBar({
  appInfo,
  syncMessage,
}: {
  appInfo: ApplicationInfo | null;
  syncMessage: string;
}) {
  return (
    <footer className="status-bar">
      <div className="status-group">
        <span className="status-dot" />
        <span>{appInfo ? `${appInfo.name} ${appInfo.version}` : "本地模式"}</span>
        <span className="status-divider" />
        <span className="offline-badge">离线可用</span>
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
  const [appInfo, setAppInfo] = useState<ApplicationInfo | null>(null);
  const [syncMessage, setSyncMessage] = useState("等待添加账户");
  const composeButtonRef = useRef<HTMLDivElement>(null);

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
    const openCompose = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() === "n" && !event.metaKey && !event.ctrlKey && !event.altKey) {
        const target = event.target as HTMLElement | null;
        if (target?.matches("input, textarea, [contenteditable='true']")) return;
        event.preventDefault();
        setComposeOpen(true);
      }
    };
    window.addEventListener("keydown", openCompose);
    return () => window.removeEventListener("keydown", openCompose);
  }, []);

  const closeCompose = () => {
    setComposeOpen(false);
    composeButtonRef.current?.querySelector<HTMLButtonElement>("button")?.focus();
  };

  return (
    <div className="app-frame">
      <div className="app-content" ref={composeButtonRef}>
        <Sidebar onCompose={() => setComposeOpen(true)} />
        <MessageList onSync={() => setSyncMessage("尚无可同步账户")} />
        <ReaderPane />
        {composeOpen && <ComposePanel onClose={closeCompose} />}
      </div>
      <StatusBar appInfo={appInfo} syncMessage={syncMessage} />
    </div>
  );
}
