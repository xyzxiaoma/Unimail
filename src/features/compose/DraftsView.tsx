import { useQuery } from "@tanstack/react-query";
import { getDrafts } from "../../lib/ipc/compose";
import type { ConnectedAccountSummary } from "../../lib/ipc/oauth-onboarding";

function displayTime(value: string): string {
  const timestamp = Number(value);
  return Number.isSafeInteger(timestamp)
    ? new Intl.DateTimeFormat("zh-CN", {
        month: "numeric",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      }).format(timestamp)
    : "";
}

export function DraftsView({
  accounts,
  onOpenDraft,
}: {
  accounts: ConnectedAccountSummary[];
  onOpenDraft: (draftId: string) => void;
}) {
  const drafts = useQuery({ queryKey: ["drafts"], queryFn: () => getDrafts() });
  const accountNames = new Map(accounts.map((account) => [account.id, account.email]));

  return (
    <main className="fixed-mail-view" aria-labelledby="drafts-heading">
      <header className="fixed-mail-view-header">
        <div>
          <p className="eyebrow">本地草稿</p>
          <h1 id="drafts-heading">草稿</h1>
        </div>
        <button type="button" onClick={() => void drafts.refetch()}>
          刷新
        </button>
      </header>
      {drafts.isLoading ? (
        <p className="fixed-mail-status">正在读取本地草稿…</p>
      ) : drafts.isError ? (
        <p className="fixed-mail-status">无法读取本地草稿，请稍后重试。</p>
      ) : drafts.data?.length ? (
        <ul className="fixed-mail-list" aria-label="草稿列表">
          {drafts.data.map((draft) => (
            <li key={draft.id}>
              <button type="button" onClick={() => onOpenDraft(draft.id)}>
                <span>
                  <strong>{draft.subject || "（无主题）"}</strong>
                  {draft.offlineReviewRequired && <em>需要联网后重新确认</em>}
                </span>
                <small>
                  {accountNames.get(draft.accountId) ?? "本地账户"} · {String(draft.recipientCount)}{" "}
                  位收件人 · {displayTime(draft.updatedAtMs)}
                </small>
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <div className="fixed-mail-empty">
          <h2>还没有草稿</h2>
          <p>写信内容会在停止编辑 1 秒后自动保存到本地。</p>
        </div>
      )}
    </main>
  );
}
