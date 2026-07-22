import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  decodeComposeCommandError,
  getLocalDraft,
  removeLocalDraft,
  saveLocalDraft,
  submitLocalDraft,
  type DraftV1,
} from "../../lib/ipc/compose";
import { ComposePanel } from "./ComposePanel";
import { protectDesktopClose } from "../../lib/ipc/window-lifecycle";

vi.mock("../../lib/ipc/compose", () => ({
  decodeComposeCommandError: vi.fn(),
  getLocalDraft: vi.fn(),
  removeLocalDraft: vi.fn(),
  saveLocalDraft: vi.fn(),
  submitLocalDraft: vi.fn(),
}));

vi.mock("../../lib/ipc/window-lifecycle", () => ({
  protectDesktopClose: vi.fn().mockResolvedValue(vi.fn()),
}));

const accountId = "00000000-0000-4000-8000-000000000001";
const draftId = "00000000-0000-4000-8000-000000000002";
const account = {
  id: accountId,
  provider: "gmail" as const,
  email: "owner@example.test",
  displayName: "测试账户",
  authState: "connected" as const,
};

function savedDraft(overrides: Partial<DraftV1> = {}): DraftV1 {
  return {
    id: draftId,
    accountId,
    to: [{ displayName: null, address: "recipient@example.test" }],
    cc: [],
    bcc: [],
    subject: "测试主题",
    plainBody: "测试正文",
    reply: false,
    revision: "1",
    createdAtMs: "10",
    updatedAtMs: "11",
    offlineReviewRequired: false,
    ...overrides,
  };
}

describe("ComposePanel", () => {
  beforeEach(() => {
    vi.mocked(decodeComposeCommandError).mockReset();
    vi.mocked(getLocalDraft).mockReset();
    vi.mocked(removeLocalDraft).mockReset();
    vi.mocked(saveLocalDraft).mockReset();
    vi.mocked(submitLocalDraft).mockReset();
    vi.mocked(protectDesktopClose).mockClear();
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("空白未编辑窗口关闭时不创建草稿", async () => {
    const onClose = vi.fn();
    render(<ComposePanel accounts={[account]} draftId={null} onClose={onClose} onSent={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "关闭写邮件窗口" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    expect(saveLocalDraft).not.toHaveBeenCalled();
  });

  it("停止编辑一秒后自动保存有意义内容", async () => {
    vi.useFakeTimers();
    vi.mocked(saveLocalDraft).mockResolvedValue(savedDraft());
    render(<ComposePanel accounts={[account]} draftId={null} onClose={vi.fn()} onSent={vi.fn()} />);

    fireEvent.change(screen.getByPlaceholderText("name@example.com，多个地址用逗号分隔"), {
      target: { value: "recipient@example.test" },
    });
    fireEvent.change(screen.getByPlaceholderText("输入邮件正文"), {
      target: { value: "测试正文" },
    });
    await vi.advanceTimersByTimeAsync(1_000);

    expect(saveLocalDraft).toHaveBeenCalledWith(
      expect.objectContaining({
        draftId: null,
        accountId,
        to: [{ displayName: null, address: "recipient@example.test" }],
        plainBody: "测试正文",
        expectedRevision: null,
      }),
    );
  });

  it("回复草稿锁定原账户且离线发送只保留草稿", async () => {
    const reply = savedDraft({ reply: true, subject: "Re: 原主题" });
    vi.mocked(getLocalDraft).mockResolvedValue(reply);
    vi.mocked(saveLocalDraft).mockResolvedValue({ ...reply, revision: "2" });
    vi.mocked(submitLocalDraft).mockResolvedValue({
      state: "offline_saved",
      draft: { ...reply, revision: "3", offlineReviewRequired: true },
      attemptId: null,
      errorCode: null,
    });
    render(
      <ComposePanel accounts={[account]} draftId={draftId} onClose={vi.fn()} onSent={vi.fn()} />,
    );

    const sender = await screen.findByRole("combobox", { name: "发件账户" });
    expect(sender).toBeDisabled();
    expect(screen.queryByRole("button", { name: "回复全部" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "发送" }));

    await waitFor(() => expect(submitLocalDraft).toHaveBeenCalledOnce());
    expect(await screen.findByText("当前离线，邮件未发送，已保留为草稿。")).toBeTruthy();
  });

  it("桌面窗口关闭前先刷新当前草稿", async () => {
    vi.mocked(saveLocalDraft).mockResolvedValue(savedDraft());
    const onClose = vi.fn();
    render(<ComposePanel accounts={[account]} draftId={null} onClose={onClose} onSent={vi.fn()} />);
    fireEvent.change(screen.getByPlaceholderText("输入邮件正文"), {
      target: { value: "退出前保存" },
    });
    await waitFor(() => expect(protectDesktopClose).toHaveBeenCalledOnce());
    const flushBeforeClose = vi.mocked(protectDesktopClose).mock.calls[0]?.[0];
    expect(flushBeforeClose).toBeDefined();

    await flushBeforeClose?.();
    expect(saveLocalDraft).toHaveBeenCalledWith(
      expect.objectContaining({ plainBody: "退出前保存" }),
    );
    expect(onClose).toHaveBeenCalledOnce();
  });
});
