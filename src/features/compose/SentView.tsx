import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  getSentItems,
  permitOutboundRetry,
  refreshLocalSentItems,
  type SentItemV1,
} from "../../lib/ipc/compose";
import type { ConnectedAccountSummary } from "../../lib/ipc/oauth-onboarding";

function stateLabel(item: SentItemV1): string {
  if (item.state === "reconciled") return "已发送";
  if (item.state === "unknown_locked") {
    return item.retryAuthorized ? "已允许再次发送" : "发送结果待确认";
  }
  return "等待邮箱确认";
}

function displayTime(value: string): string {
  const timestamp = Number(value);
  return Number.isSafeInteger(timestamp)
    ? new Intl.DateTimeFormat("zh-CN", { dateStyle: "medium", timeStyle: "short" }).format(
        timestamp,
      )
    : "";
}

export function SentView({
  accounts,
  onOpenDraft,
}: {
  accounts: ConnectedAccountSummary[];
  onOpenDraft: (draftId: string) => void;
}) {
  const sent = useQuery({ queryKey: ["sent-items"], queryFn: () => getSentItems() });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [feedback, setFeedback] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const selected =
    sent.data?.find((item) => item.attemptId === selectedId) ?? sent.data?.[0] ?? null;

  const refresh = async () => {
    if (refreshing) return;
    setRefreshing(true);
    setFeedback("");
    try {
      for (const account of accounts.filter((value) => value.authState === "connected")) {
        await refreshLocalSentItems(account.id);
      }
      await sent.refetch();
      setFeedback("已检查本地已发送状态。");
    } catch {
      setFeedback("暂时无法刷新已发送状态。");
    } finally {
      setRefreshing(false);
    }
  };

  const authorize = async (item: SentItemV1) => {
    if (
      !window.confirm(
        "我已检查邮箱的已发送邮件，仍未找到这封邮件。确认承担重复发送风险并允许再次发送吗？",
      )
    )
      return;
    try {
      const result = await permitOutboundRetry(item.attemptId);
      if (!result.authorized) {
        setFeedback("请先手动刷新一次已发送邮件。");
        return;
      }
      await sent.refetch();
      onOpenDraft(item.draftId);
    } catch {
      setFeedback("无法解除再次发送锁定，请稍后重试。");
    }
  };

  return (
    <main className="sent-workspace" aria-labelledby="sent-heading">
      <section className="sent-list-pane">
        <header className="fixed-mail-view-header">
          <div>
            <p className="eyebrow">本地发送记录</p>
            <h1 id="sent-heading">已发送</h1>
          </div>
          <button type="button" disabled={refreshing} onClick={() => void refresh()}>
            {refreshing ? "正在刷新…" : "刷新已发送"}
          </button>
        </header>
        <p className="sent-feedback" aria-live="polite">
          {feedback}
        </p>
        {sent.isLoading ? (
          <p className="fixed-mail-status">正在读取发送记录…</p>
        ) : sent.isError ? (
          <p className="fixed-mail-status">无法读取发送记录。</p>
        ) : sent.data?.length ? (
          <ul className="fixed-mail-list" aria-label="已发送列表">
            {sent.data.map((item) => (
              <li key={item.attemptId}>
                <button
                  type="button"
                  aria-current={selected?.attemptId === item.attemptId ? "true" : undefined}
                  onClick={() => setSelectedId(item.attemptId)}
                >
                  <span>
                    <strong>{item.subject || "（无主题）"}</strong>
                    <em>{stateLabel(item)}</em>
                  </span>
                  <small>
                    {item.to.map((address) => address.address).join("、") || "无收件人"}
                  </small>
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <div className="fixed-mail-empty">
            <h2>还没有发送记录</h2>
            <p>通过 Unimail 发送的邮件会显示在这里。</p>
          </div>
        )}
      </section>
      <section className="sent-detail-pane" aria-label="已发送详情">
        {selected ? (
          <>
            <header>
              <p className="eyebrow">{stateLabel(selected)}</p>
              <h2>{selected.subject || "（无主题）"}</h2>
              <p>发件人：{selected.sender.address}</p>
              <p>收件人：{selected.to.map((address) => address.address).join("、")}</p>
              <time>{displayTime(selected.updatedAtMs)}</time>
            </header>
            {selected.state === "accepted_pending" && (
              <p className="sent-notice">
                邮箱已接受处理，正在等待远端已发送记录确认；这不是伪造的服务器邮件。
              </p>
            )}
            {selected.state === "unknown_locked" && !selected.retryAuthorized && (
              <div className="sent-warning">
                <p>网络可能在提交后中断。请先刷新并检查邮箱的已发送邮件，避免重复发送。</p>
                <button
                  type="button"
                  disabled={!selected.canAuthorizeRetry}
                  onClick={() => void authorize(selected)}
                >
                  我已检查，仍要再次发送
                </button>
              </div>
            )}
            <pre className="plain-message-body">{selected.plainBody || "（无正文）"}</pre>
          </>
        ) : (
          <div className="fixed-mail-empty">
            <h2>选择一封发送记录</h2>
          </div>
        )}
      </section>
    </main>
  );
}
