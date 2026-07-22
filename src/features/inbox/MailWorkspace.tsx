import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
  type InfiniteData,
} from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { mailReaderContent } from "../../content/mail-reader.zh-CN";
import { openExternalLink } from "../../lib/ipc/external-link";
import type { ConnectedAccountSummary } from "../../lib/ipc/oauth-onboarding";
import {
  getInboxPage,
  getMailMessageDetail,
  setMailMessageRead,
  type InboxMessageSummaryV1,
  type InboxPageV1,
} from "../../lib/ipc/mail-reader";
import { SafeHtmlMessage } from "../reader/SafeHtmlMessage";

const PAGE_SIZE = 50;

function displayTime(timestamp: string): string {
  const value = Number(timestamp);
  if (!Number.isSafeInteger(value)) return "";
  return new Intl.DateTimeFormat("zh-CN", {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(value));
}

function senderLabel(message: InboxMessageSummaryV1): string {
  return message.senderName ?? message.senderAddress ?? "未知发件人";
}

function ReaderPane({
  messageId,
  account,
  onExternalLink,
  onReply,
}: {
  messageId: string | null;
  account: ConnectedAccountSummary | null;
  onExternalLink: (url: string) => void;
  onReply: (messageId: string) => void;
}) {
  const detail = useQuery({
    queryKey: ["message-detail", messageId],
    queryFn: () => getMailMessageDetail(messageId ?? ""),
    enabled: messageId !== null,
  });
  if (!messageId) {
    return (
      <section className="reader-pane" aria-labelledby="reader-heading">
        <div className="reader-empty">
          <h2 id="reader-heading">{mailReaderContent.selectMessage}</h2>
          <p>使用鼠标或 J / K 键浏览邮件。</p>
        </div>
      </section>
    );
  }
  if (detail.isLoading) {
    return (
      <section className="reader-pane reader-status">{mailReaderContent.loadingDetail}</section>
    );
  }
  if (detail.isError || !detail.data) {
    return (
      <section className="reader-pane reader-status">
        <p>{mailReaderContent.unavailable}</p>
        <button type="button" onClick={() => void detail.refetch()}>
          {mailReaderContent.retry}
        </button>
      </section>
    );
  }
  const message = detail.data;
  const from = message.addresses.find((address) => address.role === "from");
  const recipients = message.addresses.filter((address) => address.role === "to");
  return (
    <section className="reader-pane reader-content" aria-labelledby="reader-heading">
      <header className="reader-message-header">
        <div className="reader-heading-row">
          <div>
            <p className="eyebrow">{account?.email ?? mailReaderContent.cached}</p>
            <h2 id="reader-heading">{message.summary.subject || "（无主题）"}</h2>
          </div>
          <button type="button" className="reader-reply-button" onClick={() => onReply(messageId)}>
            回复
          </button>
        </div>
        <div className="reader-addresses">
          <strong>{from?.displayName ?? from?.address ?? senderLabel(message.summary)}</strong>
          {from?.address && <span>{from.address}</span>}
          {recipients.length > 0 && (
            <small>收件人：{recipients.map((address) => address.address).join("、")}</small>
          )}
          <time>{displayTime(message.summary.receivedAtMs)}</time>
        </div>
      </header>
      <div className="reader-body">
        {message.htmlBody ? (
          <SafeHtmlMessage
            key={message.summary.id}
            messageId={message.summary.id}
            html={message.htmlBody}
            onExternalLink={onExternalLink}
          />
        ) : message.plainBody ? (
          <pre className="plain-message-body">{message.plainBody}</pre>
        ) : (
          <p>{mailReaderContent.noBody}</p>
        )}
        {message.attachments.length > 0 && (
          <section className="reader-attachments" aria-labelledby="attachment-heading">
            <h3 id="attachment-heading">附件（{message.attachments.length}）</h3>
            <ul>
              {message.attachments.map((attachment) => (
                <li key={attachment.id}>{attachment.fileName ?? "未命名附件"}</li>
              ))}
            </ul>
          </section>
        )}
      </div>
    </section>
  );
}

export function MailWorkspace({
  accounts,
  onAddAccount,
  onReply,
  onSync,
}: {
  accounts: ConnectedAccountSummary[];
  onAddAccount: (opener: HTMLButtonElement) => void;
  onReply: (messageId: string) => void;
  onSync: () => void;
}) {
  const [accountId, setAccountId] = useState<string | null>(null);
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [externalUrl, setExternalUrl] = useState<string | null>(null);
  const [externalLinkError, setExternalLinkError] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const sentinelRef = useRef<HTMLDivElement>(null);
  const nextPageInFlightRef = useRef(false);
  const queryClient = useQueryClient();
  const inbox = useInfiniteQuery({
    queryKey: ["inbox", accountId, unreadOnly],
    initialPageParam: null as string | null,
    queryFn: ({ pageParam }) =>
      getInboxPage({ accountId, unreadOnly, cursor: pageParam, limit: PAGE_SIZE }),
    getNextPageParam: (page) => page.nextCursor ?? undefined,
  });
  const messages = useMemo(() => {
    const seen = new Set<string>();
    return (
      inbox.data?.pages
        .flatMap((page) => page.items)
        .filter((message) => (seen.has(message.id) ? false : (seen.add(message.id), true))) ?? []
    );
  }, [inbox.data]);
  const accountsById = useMemo(
    () => new Map(accounts.map((account) => [account.id, account])),
    [accounts],
  );
  const selected = messages.find((message) => message.id === selectedId) ?? null;
  // eslint-disable-next-line react-hooks/incompatible-library -- virtualization is isolated to this list owner.
  const virtualizer = useVirtualizer({
    count: messages.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 92,
    overscan: 6,
  });
  const virtualItems = virtualizer.getVirtualItems();
  const fetchNextPage = inbox.fetchNextPage;
  const requestNextPage = useCallback(() => {
    if (!inbox.hasNextPage || nextPageInFlightRef.current) return;
    nextPageInFlightRef.current = true;
    void fetchNextPage().finally(() => {
      nextPageInFlightRef.current = false;
    });
  }, [fetchNextPage, inbox.hasNextPage]);
  const readMutation = useMutation({
    mutationFn: (messageId: string) => setMailMessageRead(messageId, true),
    onMutate: async (messageId) => {
      const key = ["inbox", accountId, unreadOnly] as const;
      await queryClient.cancelQueries({ queryKey: key });
      const previous = queryClient.getQueryData<InfiniteData<InboxPageV1>>(key);
      queryClient.setQueryData<InfiniteData<InboxPageV1>>(key, (current) =>
        current
          ? {
              ...current,
              pages: current.pages.map((page) => ({
                ...page,
                items: page.items.map((item) =>
                  item.id === messageId ? { ...item, read: true } : item,
                ),
              })),
            }
          : current,
      );
      return { key, previous };
    },
    onError: (_error, _messageId, context) => {
      if (context?.previous) queryClient.setQueryData(context.key, context.previous);
    },
  });
  const mutateRead = readMutation.mutate;

  useEffect(() => {
    if (selectedId && messages.some((message) => message.id === selectedId)) return;
    setSelectedId(messages[0]?.id ?? null);
  }, [messages, selectedId]);

  useEffect(() => {
    if (!selected || selected.read) return;
    const messageId = selected.id;
    const timer = window.setTimeout(() => mutateRead(messageId), 800);
    return () => window.clearTimeout(timer);
  }, [mutateRead, selected]);

  useEffect(() => {
    const handleKeys = (event: KeyboardEvent) => {
      if (event.key !== "j" && event.key !== "J" && event.key !== "k" && event.key !== "K") return;
      if (
        event.target instanceof Element &&
        event.target.matches("input, textarea, [contenteditable='true']")
      )
        return;
      const index = messages.findIndex((message) => message.id === selectedId);
      const nextIndex =
        event.key.toLowerCase() === "j"
          ? Math.min(index + 1, messages.length - 1)
          : Math.max(index - 1, 0);
      const next = messages[nextIndex];
      if (!next) return;
      event.preventDefault();
      setSelectedId(next.id);
      if (nextIndex >= messages.length - 3 && inbox.hasNextPage && !inbox.isFetchingNextPage) {
        requestNextPage();
      }
    };
    window.addEventListener("keydown", handleKeys);
    return () => window.removeEventListener("keydown", handleKeys);
  }, [inbox.hasNextPage, inbox.isFetchingNextPage, messages, requestNextPage, selectedId]);

  useEffect(() => {
    const target = sentinelRef.current;
    if (!target || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver((entries) => {
      if (
        entries.some((entry) => entry.isIntersecting) &&
        inbox.hasNextPage &&
        !inbox.isFetchingNextPage
      ) {
        requestNextPage();
      }
    });
    observer.observe(target);
    return () => observer.disconnect();
  }, [inbox.hasNextPage, inbox.isFetchingNextPage, requestNextPage]);

  const listRows =
    virtualItems.length > 0
      ? virtualItems
      : messages.map((_, index) => ({ index, start: index * 92, size: 92, key: index }));
  return (
    <>
      <section className="message-pane" aria-labelledby="inbox-heading">
        <header className="message-header">
          <div>
            <select
              aria-label="选择邮箱范围"
              value={accountId ?? ""}
              onChange={(event) => {
                setAccountId(event.target.value || null);
                setSelectedId(null);
              }}
            >
              <option value="">{mailReaderContent.allAccounts}</option>
              {accounts.map((account) => (
                <option key={account.id} value={account.id}>
                  {account.displayName ?? account.email}
                </option>
              ))}
            </select>
            <h1 id="inbox-heading">{mailReaderContent.inboxTitle}</h1>
          </div>
          <button className="icon-button" type="button" onClick={onSync} aria-label="同步邮件">
            ↻
          </button>
        </header>
        <div className="filter-row">
          <div role="group" aria-label="邮件筛选">
            <button
              className={!unreadOnly ? "filter active" : "filter"}
              type="button"
              aria-pressed={!unreadOnly}
              onClick={() => {
                setUnreadOnly(false);
                setSelectedId(null);
              }}
            >
              {mailReaderContent.allMessages}
            </button>
            <button
              className={unreadOnly ? "filter active" : "filter"}
              type="button"
              aria-pressed={unreadOnly}
              onClick={() => {
                setUnreadOnly(true);
                setSelectedId(null);
              }}
            >
              {mailReaderContent.unreadMessages}
            </button>
          </div>
          <span className="cache-label">{mailReaderContent.cached}</span>
        </div>
        {inbox.isLoading ? (
          <div className="inbox-status">{mailReaderContent.loading}</div>
        ) : inbox.isError && messages.length === 0 ? (
          <div className="inbox-status">
            <p>{mailReaderContent.unavailable}</p>
            <button type="button" onClick={() => void inbox.refetch()}>
              {mailReaderContent.retry}
            </button>
          </div>
        ) : messages.length === 0 ? (
          <div className="empty-list">
            <h2>{unreadOnly ? mailReaderContent.emptyUnread : mailReaderContent.empty}</h2>
            <p>{mailReaderContent.emptyDescription}</p>
            {accounts.length === 0 && (
              <button
                className="secondary-action"
                onClick={(event) => onAddAccount(event.currentTarget)}
                type="button"
              >
                添加邮箱账户
              </button>
            )}
          </div>
        ) : (
          <div
            className="message-scroll"
            ref={scrollRef}
            role="listbox"
            aria-label="邮件列表"
            aria-activedescendant={selectedId ? `message-${selectedId}` : undefined}
          >
            <div
              className="message-virtual-space"
              style={{
                height: `${String(Math.max(virtualizer.getTotalSize(), messages.length * 92))}px`,
              }}
            >
              {listRows.map((row) => {
                const message = messages[row.index];
                if (!message) return null;
                const account = accountsById.get(message.accountId);
                return (
                  <button
                    id={`message-${message.id}`}
                    key={message.id}
                    type="button"
                    role="option"
                    aria-selected={message.id === selectedId}
                    className={`message-row${message.read ? "" : " unread"}${message.id === selectedId ? " selected" : ""}`}
                    style={{
                      transform: `translateY(${String(row.start)}px)`,
                      height: `${String(row.size)}px`,
                    }}
                    onClick={() => setSelectedId(message.id)}
                  >
                    <span className="message-row-top">
                      <strong>{senderLabel(message)}</strong>
                      <time>{displayTime(message.receivedAtMs)}</time>
                    </span>
                    <span className="message-subject">{message.subject || "（无主题）"}</span>
                    <span className="message-snippet">{message.snippet || "无摘要"}</span>
                    <span className="message-account">
                      {account?.email ?? "本地账户"}
                      {message.hasAttachments ? " · 有附件" : ""}
                    </span>
                  </button>
                );
              })}
            </div>
            <div ref={sentinelRef} className="load-more-sentinel" />
            {inbox.isFetchingNextPage && (
              <p className="load-more-state">{mailReaderContent.loadingMore}</p>
            )}
            {inbox.isError && messages.length > 0 && (
              <button className="load-more-retry" type="button" onClick={requestNextPage}>
                {mailReaderContent.retry}
              </button>
            )}
          </div>
        )}
      </section>
      <ReaderPane
        messageId={selectedId}
        account={selected ? (accountsById.get(selected.accountId) ?? null) : null}
        onExternalLink={setExternalUrl}
        onReply={onReply}
      />
      {externalUrl && (
        <div className="link-dialog-backdrop" role="presentation">
          <section
            className="link-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="link-dialog-title"
          >
            <h2 id="link-dialog-title">确认打开外部链接</h2>
            <strong>{new URL(externalUrl).hostname}</strong>
            <p>{externalUrl}</p>
            {externalLinkError && <p role="alert">无法打开系统浏览器，请稍后重试。</p>}
            <div>
              <button
                type="button"
                onClick={() => {
                  setExternalLinkError(false);
                  setExternalUrl(null);
                }}
              >
                取消
              </button>
              <button
                type="button"
                onClick={() => {
                  setExternalLinkError(false);
                  void openExternalLink(externalUrl)
                    .then(() => setExternalUrl(null))
                    .catch(() => setExternalLinkError(true));
                }}
              >
                打开浏览器
              </button>
            </div>
          </section>
        </div>
      )}
    </>
  );
}
