import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { composeContent } from "../../content/compose.zh-CN";
import {
  decodeComposeCommandError,
  getLocalDraft,
  removeLocalDraft,
  saveLocalDraft,
  submitLocalDraft,
  type DraftAddressV1,
} from "../../lib/ipc/compose";
import type { ConnectedAccountSummary } from "../../lib/ipc/oauth-onboarding";
import { protectDesktopClose } from "../../lib/ipc/window-lifecycle";

type ComposeForm = {
  accountId: string;
  to: string;
  cc: string;
  bcc: string;
  subject: string;
  plainBody: string;
};

function addressesFromText(value: string): DraftAddressV1[] {
  return value
    .split(/[,;\n]/u)
    .map((address) => address.trim())
    .filter((address) => address.length > 0)
    .map((address) => ({ displayName: null, address }));
}

function addressesToText(values: DraftAddressV1[]): string {
  return values.map((value) => value.address).join(", ");
}

function meaningful(form: ComposeForm): boolean {
  return [form.to, form.cc, form.bcc, form.subject, form.plainBody].some(
    (value) => value.trim().length > 0,
  );
}

function sendErrorCopy(code: string | null): string {
  if (code === "authentication_required") return composeContent.noAccount;
  if (code === "provider_unavailable") return "邮箱服务暂时不可用，草稿仍保存在本地。";
  if (code === "recipient_rejected") return "收件人被邮箱服务拒绝，请检查地址后重试。";
  return composeContent.rejected;
}

export function ComposePanel({
  accounts,
  draftId,
  onClose,
  onSent,
}: {
  accounts: ConnectedAccountSummary[];
  draftId: string | null;
  onClose: () => void;
  onSent: () => void;
}) {
  const titleId = useId();
  const availableAccounts = useMemo(
    () => accounts.filter((account) => account.authState === "connected"),
    [accounts],
  );
  const [form, setForm] = useState<ComposeForm>(() => ({
    accountId: availableAccounts[0]?.id ?? "",
    to: "",
    cc: "",
    bcc: "",
    subject: "",
    plainBody: "",
  }));
  const [loaded, setLoaded] = useState(draftId === null);
  const [reply, setReply] = useState(false);
  const [savedDraftId, setSavedDraftId] = useState<string | null>(draftId);
  const [showCopies, setShowCopies] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [status, setStatus] = useState(
    availableAccounts.length > 0 ? "" : composeContent.noAccount,
  );
  const [sending, setSending] = useState(false);
  const formRef = useRef(form);
  const draftIdRef = useRef<string | null>(draftId);
  const revisionRef = useRef<string | null>(null);
  const offlineReviewRef = useRef(false);
  const editGenerationRef = useRef(0);
  const queuedSaveRef = useRef<{ form: ComposeForm; generation: number } | null>(null);
  const saveLoopRef = useRef<Promise<void> | null>(null);

  useEffect(() => {
    formRef.current = form;
  }, [form]);

  useEffect(() => {
    const defaultAccountId = availableAccounts[0]?.id;
    if (!defaultAccountId || formRef.current.accountId) return;
    setForm((current) => ({ ...current, accountId: defaultAccountId }));
  }, [availableAccounts]);

  useEffect(() => {
    if (draftId === null) return;
    let active = true;
    void getLocalDraft(draftId)
      .then((draft) => {
        if (!active) return;
        draftIdRef.current = draft.id;
        setSavedDraftId(draft.id);
        revisionRef.current = draft.revision;
        offlineReviewRef.current = draft.offlineReviewRequired;
        setReply(draft.reply);
        setShowCopies(draft.cc.length > 0 || draft.bcc.length > 0);
        setForm({
          accountId: draft.accountId,
          to: addressesToText(draft.to),
          cc: addressesToText(draft.cc),
          bcc: addressesToText(draft.bcc),
          subject: draft.subject,
          plainBody: draft.plainBody,
        });
        setLoaded(true);
      })
      .catch(() => {
        if (active) setStatus("无法打开这封本地草稿。");
      });
    return () => {
      active = false;
    };
  }, [draftId]);

  const runSaveLoop = useCallback(async () => {
    while (queuedSaveRef.current) {
      const queued = queuedSaveRef.current;
      queuedSaveRef.current = null;
      setStatus(composeContent.saving);
      const saved = await saveLocalDraft({
        draftId: draftIdRef.current,
        accountId: queued.form.accountId,
        to: addressesFromText(queued.form.to),
        cc: addressesFromText(queued.form.cc),
        bcc: addressesFromText(queued.form.bcc),
        subject: queued.form.subject,
        plainBody: queued.form.plainBody,
        expectedRevision: revisionRef.current,
      });
      draftIdRef.current = saved.id;
      setSavedDraftId(saved.id);
      revisionRef.current = saved.revision;
      offlineReviewRef.current = saved.offlineReviewRequired;
      setReply(saved.reply);
      if (queued.generation === editGenerationRef.current) setDirty(false);
      setStatus(composeContent.saved);
    }
  }, []);

  const flush = useCallback(
    async (snapshot = formRef.current) => {
      if (!meaningful(snapshot)) return;
      queuedSaveRef.current = { form: snapshot, generation: editGenerationRef.current };
      if (!saveLoopRef.current) {
        saveLoopRef.current = runSaveLoop().finally(() => {
          saveLoopRef.current = null;
        });
      }
      await saveLoopRef.current;
    },
    [runSaveLoop],
  );

  useEffect(() => {
    if (!loaded || !dirty || !meaningful(form)) return;
    const timer = window.setTimeout(() => {
      void flush(form).catch((error: unknown) => {
        try {
          const decoded = decodeComposeCommandError(error);
          setStatus(
            decoded.code === "revision_conflict"
              ? composeContent.conflict
              : composeContent.saveFailed,
          );
        } catch {
          setStatus(composeContent.saveFailed);
        }
      });
    }, 1_000);
    return () => window.clearTimeout(timer);
  }, [dirty, flush, form, loaded]);

  const flushAndClose = useCallback(async () => {
    await flush();
    onClose();
  }, [flush, onClose]);

  const close = useCallback(async () => {
    try {
      await flushAndClose();
    } catch {
      setStatus(composeContent.saveFailed);
    }
  }, [flushAndClose]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    void protectDesktopClose(flushAndClose)
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      })
      .catch(() => {
        /* Browser preview remains usable without desktop window events. */
      });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [flushAndClose]);

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        void close();
      }
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [close]);

  const update = <K extends keyof ComposeForm>(key: K, value: ComposeForm[K]) => {
    editGenerationRef.current += 1;
    setForm((current) => ({ ...current, [key]: value }));
    setDirty(true);
  };

  const send = async () => {
    if (sending) return;
    setSending(true);
    try {
      await flush();
      const currentDraftId = draftIdRef.current;
      const currentRevision = revisionRef.current;
      if (!currentDraftId || !currentRevision) {
        setStatus("请先填写邮件内容。");
        return;
      }
      const emptySubjectConfirmed =
        formRef.current.subject.trim().length > 0 ||
        window.confirm(composeContent.emptySubjectConfirm);
      if (!emptySubjectConfirmed) return;
      const offlineReviewConfirmed =
        !offlineReviewRef.current || window.confirm(composeContent.offlineReviewConfirm);
      if (!offlineReviewConfirmed) return;
      setStatus(composeContent.sending);
      const result = await submitLocalDraft({
        draftId: currentDraftId,
        draftRevision: currentRevision,
        emptySubjectConfirmed,
        offlineReviewConfirmed,
      });
      if (result.state === "offline_saved" && result.draft) {
        draftIdRef.current = result.draft.id;
        setSavedDraftId(result.draft.id);
        revisionRef.current = result.draft.revision;
        offlineReviewRef.current = true;
        setStatus(composeContent.offlineSaved);
      } else if (result.state === "accepted_pending") {
        setStatus(composeContent.accepted);
        onSent();
        onClose();
      } else if (result.state === "unknown_locked") {
        setStatus(composeContent.unknown);
        onSent();
        onClose();
      } else {
        setStatus(sendErrorCopy(result.errorCode));
      }
    } catch (error: unknown) {
      try {
        setStatus(decodeComposeCommandError(error).message);
      } catch {
        setStatus(composeContent.rejected);
      }
    } finally {
      setSending(false);
    }
  };

  const remove = async () => {
    const currentDraftId = draftIdRef.current;
    if (!currentDraftId || !window.confirm(composeContent.deleteConfirm)) return;
    try {
      await removeLocalDraft(currentDraftId);
      onClose();
    } catch {
      setStatus("无法删除本地草稿，请稍后重试。");
    }
  };

  return (
    <section className="compose-panel" role="dialog" aria-labelledby={titleId} aria-modal="false">
      <header>
        <div>
          <p className="eyebrow">
            {reply ? composeContent.replyEyebrow : composeContent.newEyebrow}
          </p>
          <h2 id={titleId}>{composeContent.title}</h2>
        </div>
        <button
          type="button"
          className="icon-button"
          onClick={() => void close()}
          aria-label={composeContent.close}
        >
          ×
        </button>
      </header>
      {!loaded ? (
        <div className="compose-loading">正在打开本地草稿…</div>
      ) : (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void send();
          }}
          onBlur={() => {
            if (dirty) void flush().catch(() => setStatus(composeContent.saveFailed));
          }}
        >
          <label>
            {composeContent.sender}
            <select
              value={form.accountId}
              disabled={reply}
              onChange={(event) => update("accountId", event.currentTarget.value)}
            >
              {availableAccounts.map((account) => (
                <option key={account.id} value={account.id}>
                  {account.displayName ?? account.email} · {account.email}
                </option>
              ))}
            </select>
          </label>
          <label>
            {composeContent.to}
            <input
              autoFocus
              type="text"
              value={form.to}
              placeholder="name@example.com，多个地址用逗号分隔"
              onChange={(event) => update("to", event.currentTarget.value)}
            />
          </label>
          {!showCopies && (
            <button
              className="compose-copy-toggle"
              type="button"
              onClick={() => setShowCopies(true)}
            >
              {composeContent.addCcBcc}
            </button>
          )}
          {showCopies && (
            <div className="compose-copy-fields">
              <label>
                {composeContent.cc}
                <input
                  type="text"
                  value={form.cc}
                  onChange={(event) => update("cc", event.currentTarget.value)}
                />
              </label>
              <label>
                {composeContent.bcc}
                <input
                  type="text"
                  value={form.bcc}
                  onChange={(event) => update("bcc", event.currentTarget.value)}
                />
              </label>
            </div>
          )}
          <label>
            {composeContent.subject}
            <input
              type="text"
              value={form.subject}
              placeholder="邮件主题"
              onChange={(event) => update("subject", event.currentTarget.value)}
            />
          </label>
          <label className="body-field">
            <span className="sr-only">{composeContent.body}</span>
            <textarea
              value={form.plainBody}
              placeholder="输入邮件正文"
              onChange={(event) => update("plainBody", event.currentTarget.value)}
            />
          </label>
          <footer>
            <span aria-live="polite">{status}</span>
            <div className="compose-actions">
              {savedDraftId && (
                <button className="compose-delete" type="button" onClick={() => void remove()}>
                  {composeContent.deleteDraft}
                </button>
              )}
              <button type="submit" disabled={sending || availableAccounts.length === 0}>
                {sending ? composeContent.sending : composeContent.send}
              </button>
            </div>
          </footer>
        </form>
      )}
    </section>
  );
}
