import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { openExternalLink } from "../../lib/ipc/external-link";
import {
  beginMailAttachmentDownload,
  cancelMailAttachmentDownload,
  getInboxPage,
  getMailAttachmentDownloadStatus,
  getMailMessageDetail,
  getSearchPage,
  setMailMessageRead,
} from "../../lib/ipc/mail-reader";
import { MailWorkspace } from "./MailWorkspace";

vi.mock("../../lib/ipc/mail-reader", () => ({
  beginMailAttachmentDownload: vi.fn(),
  cancelMailAttachmentDownload: vi.fn(),
  getInboxPage: vi.fn(),
  getMailAttachmentDownloadStatus: vi.fn(),
  getMailMessageDetail: vi.fn(),
  getSearchPage: vi.fn(),
  setMailMessageRead: vi.fn(),
}));

vi.mock("../../lib/ipc/external-link", () => ({
  openExternalLink: vi.fn(),
}));

const messageId = "00000000-0000-4000-8000-000000000001";
const secondMessageId = "00000000-0000-4000-8000-000000000004";
const accountId = "00000000-0000-4000-8000-000000000002";
const mailboxId = "00000000-0000-4000-8000-000000000003";
const attachmentId = "00000000-0000-4000-8000-000000000005";
const secondAttachmentId = "00000000-0000-4000-8000-000000000006";
const operationId = "00000000-0000-4000-8000-000000000007";
const retryOperationId = "00000000-0000-4000-8000-000000000008";
const summary = {
  id: messageId,
  accountId,
  mailboxId,
  subject: "统一收件箱测试",
  snippet: "虚构邮件摘要",
  senderName: "测试发件人",
  senderAddress: "sender@example.test",
  read: false,
  direction: "incoming" as const,
  sentAtMs: null,
  receivedAtMs: "42",
  hasAttachments: false,
};

function renderWorkspace(onReply = vi.fn()) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MailWorkspace
        accounts={[
          {
            id: accountId,
            provider: "gmail",
            email: "owner@example.test",
            displayName: "测试账户",
            authState: "connected",
          },
        ]}
        onAddAccount={vi.fn()}
        onReply={onReply}
        onSync={vi.fn()}
      />
    </QueryClientProvider>,
  );
}

function attachmentDetail(attachments: Array<{ id: string; fileName: string }>) {
  const readSummary = { ...summary, read: true, hasAttachments: true };
  return {
    summary: readSummary,
    threadId: null,
    rfcMessageId: null,
    plainBody: "附件测试正文",
    htmlBody: null,
    parserVersion: 1,
    sanitizerVersion: 1,
    addresses: [],
    attachments: attachments.map((attachment) => ({
      ...attachment,
      mediaType: "text/plain",
      sizeBytes: "12",
      contentId: null,
      inline: false,
    })),
  };
}

describe("MailWorkspace", () => {
  beforeEach(() => {
    vi.mocked(getInboxPage).mockReset();
    vi.mocked(getSearchPage).mockReset();
    vi.mocked(getMailMessageDetail).mockReset();
    vi.mocked(setMailMessageRead).mockReset();
    vi.mocked(beginMailAttachmentDownload).mockReset();
    vi.mocked(getMailAttachmentDownloadStatus).mockReset();
    vi.mocked(cancelMailAttachmentDownload).mockReset();
    vi.mocked(openExternalLink).mockReset();
  });
  afterEach(cleanup);
  afterEach(() => vi.unstubAllGlobals());

  it("展示本地邮件详情并在稳定选择 800ms 后写入已读状态", async () => {
    vi.mocked(getInboxPage).mockResolvedValue({ items: [summary], nextCursor: null });
    vi.mocked(getMailMessageDetail).mockResolvedValue({
      summary,
      threadId: null,
      rfcMessageId: null,
      plainBody: "这是虚构的本地缓存正文。",
      htmlBody: null,
      parserVersion: 1,
      sanitizerVersion: 1,
      addresses: [
        {
          role: "from",
          position: 0,
          displayName: "测试发件人",
          address: "sender@example.test",
        },
      ],
      attachments: [],
    });
    vi.mocked(setMailMessageRead).mockResolvedValue({
      messageId,
      read: true,
      generation: "1",
    });
    renderWorkspace();

    expect(await screen.findByRole("option", { name: /测试发件人/ })).toBeTruthy();
    expect(await screen.findByText("这是虚构的本地缓存正文。")).toBeTruthy();
    await waitFor(() => expect(setMailMessageRead).toHaveBeenCalledWith(messageId, true), {
      timeout: 1_500,
    });
  });

  it("从阅读器提供单一回复入口并传递本地邮件 ID", async () => {
    const readSummary = { ...summary, read: true };
    const onReply = vi.fn();
    vi.mocked(getInboxPage).mockResolvedValue({ items: [readSummary], nextCursor: null });
    vi.mocked(getMailMessageDetail).mockResolvedValue({
      summary: readSummary,
      threadId: null,
      rfcMessageId: null,
      plainBody: "用于回复入口测试的正文。",
      htmlBody: null,
      parserVersion: 1,
      sanitizerVersion: 1,
      addresses: [],
      attachments: [],
    });
    renderWorkspace(onReply);

    fireEvent.click(await screen.findByRole("button", { name: "回复" }));
    expect(onReply).toHaveBeenCalledWith(messageId);
    expect(screen.queryByRole("button", { name: "回复全部" })).toBeNull();
  });

  it("外部链接取消时不调用系统浏览器，确认时只打开展示的完整地址", async () => {
    const readSummary = { ...summary, read: true };
    const url = "https://docs.example.test/path?source=mail";
    vi.mocked(getInboxPage).mockResolvedValue({ items: [readSummary], nextCursor: null });
    vi.mocked(getMailMessageDetail).mockResolvedValue({
      summary: readSummary,
      threadId: null,
      rfcMessageId: null,
      plainBody: null,
      htmlBody: `<a href="${url}">查看文档</a>`,
      parserVersion: 1,
      sanitizerVersion: 1,
      addresses: [],
      attachments: [],
    });
    renderWorkspace();

    fireEvent.click(await screen.findByRole("button", { name: "查看文档" }));
    const dialog = screen.getByRole("dialog", { name: "确认打开外部链接" });
    expect(within(dialog).getByText("docs.example.test")).toBeTruthy();
    expect(within(dialog).getByText(url)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(openExternalLink).not.toHaveBeenCalled();

    vi.mocked(openExternalLink).mockResolvedValueOnce();
    fireEvent.click(screen.getByRole("button", { name: "查看文档" }));
    fireEvent.click(screen.getByRole("button", { name: "打开浏览器" }));
    await waitFor(() => expect(openExternalLink).toHaveBeenCalledWith(url));
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "确认打开外部链接" })).toBeNull(),
    );

    vi.mocked(openExternalLink).mockRejectedValueOnce(new Error("fictional opener failure"));
    fireEvent.click(screen.getByRole("button", { name: "查看文档" }));
    fireEvent.click(screen.getByRole("button", { name: "打开浏览器" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("无法打开系统浏览器，请稍后重试。");
  });

  it("快速使用 J 切换邮件时取消旧的 800ms 已读计时", async () => {
    const secondSummary = {
      ...summary,
      id: secondMessageId,
      subject: "第二封邮件",
      receivedAtMs: "41",
    };
    vi.mocked(getInboxPage).mockResolvedValue({
      items: [summary, secondSummary],
      nextCursor: null,
    });
    vi.mocked(getMailMessageDetail).mockImplementation((id) =>
      Promise.resolve({
        summary: id === messageId ? summary : secondSummary,
        threadId: null,
        rfcMessageId: null,
        plainBody: id === messageId ? "第一封正文" : "第二封正文",
        htmlBody: null,
        parserVersion: 1,
        sanitizerVersion: 1,
        addresses: [],
        attachments: [],
      }),
    );
    vi.mocked(setMailMessageRead).mockResolvedValue({
      messageId: secondMessageId,
      read: true,
      generation: "1",
    });
    renderWorkspace();

    expect(await screen.findByText("第一封正文")).toBeTruthy();
    fireEvent.keyDown(window, { key: "j" });
    expect(await screen.findByText("第二封正文")).toBeTruthy();
    await waitFor(() => expect(setMailMessageRead).toHaveBeenCalledWith(secondMessageId, true), {
      timeout: 1_500,
    });
    expect(setMailMessageRead).toHaveBeenCalledTimes(1);
    expect(setMailMessageRead).not.toHaveBeenCalledWith(messageId, true);
  });

  it("列表接近底部时只自动请求一次下一页并保留已有邮件", async () => {
    let intersectionCallback!: IntersectionObserverCallback;
    class FakeIntersectionObserver implements IntersectionObserver {
      readonly root = null;
      readonly rootMargin = "0px";
      readonly thresholds = [0];
      constructor(callback: IntersectionObserverCallback) {
        intersectionCallback = callback;
      }
      disconnect() {}
      observe() {}
      takeRecords(): IntersectionObserverEntry[] {
        return [];
      }
      unobserve() {}
    }
    vi.stubGlobal("IntersectionObserver", FakeIntersectionObserver);
    const firstSummary = { ...summary, read: true };
    const secondSummary = {
      ...summary,
      id: secondMessageId,
      subject: "下一页邮件",
      receivedAtMs: "41",
      read: true,
    };
    vi.mocked(getInboxPage).mockImplementation((request) =>
      Promise.resolve(
        request.cursor
          ? { items: [secondSummary], nextCursor: null }
          : { items: [firstSummary], nextCursor: "v1:42:next" },
      ),
    );
    vi.mocked(getMailMessageDetail).mockImplementation((id) =>
      Promise.resolve({
        summary: id === messageId ? firstSummary : secondSummary,
        threadId: null,
        rfcMessageId: null,
        plainBody: "分页测试正文",
        htmlBody: null,
        parserVersion: 1,
        sanitizerVersion: 1,
        addresses: [],
        attachments: [],
      }),
    );
    renderWorkspace();

    expect(await screen.findByRole("option", { name: /统一收件箱测试/ })).toBeTruthy();
    const entry = { isIntersecting: true } as IntersectionObserverEntry;
    intersectionCallback([entry], {} as IntersectionObserver);
    intersectionCallback([entry], {} as IntersectionObserver);
    expect(await screen.findByRole("option", { name: /下一页邮件/ })).toBeTruthy();
    expect(screen.getByRole("option", { name: /统一收件箱测试/ })).toBeTruthy();
    expect(getInboxPage).toHaveBeenCalledTimes(2);
    expect(getInboxPage).toHaveBeenLastCalledWith({
      accountId: null,
      unreadOnly: false,
      cursor: "v1:42:next",
      limit: 50,
    });
  });

  it("按当前账户和未读范围搜索本地邮件并打开结果", async () => {
    const readSummary = { ...summary, read: true };
    vi.mocked(getInboxPage).mockResolvedValue({ items: [readSummary], nextCursor: null });
    vi.mocked(getSearchPage).mockResolvedValue({
      items: [{ summary: readSummary, matchContext: "正文中的项目进展" }],
      nextCursor: null,
    });
    vi.mocked(getMailMessageDetail).mockResolvedValue({
      summary: readSummary,
      threadId: null,
      rfcMessageId: null,
      plainBody: "搜索结果正文",
      htmlBody: null,
      parserVersion: 1,
      sanitizerVersion: 1,
      addresses: [],
      attachments: [],
    });
    renderWorkspace();

    fireEvent.change(screen.getByRole("searchbox", { name: "搜索邮件" }), {
      target: { value: "项目" },
    });
    expect(await screen.findByText("正文中的项目进展")).toBeTruthy();
    expect(getSearchPage).toHaveBeenCalledWith({
      query: "项目",
      accountId: null,
      unreadOnly: false,
      cursor: null,
      limit: 50,
    });
    expect(await screen.findByText("搜索结果正文")).toBeTruthy();
  });

  it("清除搜索后恢复本地收件箱且不提交空查询", async () => {
    const searchSummary = {
      ...summary,
      id: secondMessageId,
      subject: "本地搜索结果",
      snippet: "搜索摘要",
      read: true,
    };
    vi.mocked(getInboxPage).mockResolvedValue({
      items: [{ ...summary, read: true }],
      nextCursor: null,
    });
    vi.mocked(getSearchPage).mockResolvedValue({
      items: [{ summary: searchSummary, matchContext: "正文命中内容" }],
      nextCursor: null,
    });
    vi.mocked(getMailMessageDetail).mockImplementation((id) =>
      Promise.resolve({
        summary: id === secondMessageId ? searchSummary : { ...summary, read: true },
        threadId: null,
        rfcMessageId: null,
        plainBody: id === secondMessageId ? "搜索结果正文" : "原收件箱正文",
        htmlBody: null,
        parserVersion: 1,
        sanitizerVersion: 1,
        addresses: [],
        attachments: [],
      }),
    );
    renderWorkspace();

    const searchbox = screen.getByRole("searchbox", { name: "搜索邮件" });
    fireEvent.change(searchbox, { target: { value: "项目" } });
    expect(await screen.findByText("正文命中内容")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "清除" }));

    expect(await screen.findByRole("option", { name: /统一收件箱测试/ })).toBeTruthy();
    expect(getSearchPage).toHaveBeenCalledTimes(1);
    expect(getSearchPage).not.toHaveBeenCalledWith(expect.objectContaining({ query: "" }));
  });

  it("附件保存选择被取消时恢复空闲且不显示失败", async () => {
    const readSummary = { ...summary, read: true, hasAttachments: true };
    vi.mocked(getInboxPage).mockResolvedValue({ items: [readSummary], nextCursor: null });
    vi.mocked(getMailMessageDetail).mockResolvedValue(
      attachmentDetail([{ id: attachmentId, fileName: "report.txt" }]),
    );
    vi.mocked(beginMailAttachmentDownload).mockResolvedValue(null);
    renderWorkspace();

    fireEvent.click(await screen.findByRole("button", { name: "保存" }));
    await waitFor(() => expect(beginMailAttachmentDownload).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("button", { name: "保存" })).toBeTruthy();
  });

  it("显示附件进度并在取消后立即恢复保存操作", async () => {
    const readSummary = { ...summary, read: true, hasAttachments: true };
    vi.mocked(getInboxPage).mockResolvedValue({ items: [readSummary], nextCursor: null });
    vi.mocked(getMailMessageDetail).mockResolvedValue(
      attachmentDetail([
        { id: attachmentId, fileName: "report.txt" },
        { id: secondAttachmentId, fileName: "notes.txt" },
      ]),
    );
    const downloading = {
      operationId,
      attachmentId,
      state: "downloading" as const,
      bytesWritten: "6",
      totalBytes: "12",
      error: null,
    };
    vi.mocked(beginMailAttachmentDownload).mockResolvedValue(downloading);
    vi.mocked(getMailAttachmentDownloadStatus).mockResolvedValue(downloading);
    vi.mocked(cancelMailAttachmentDownload).mockResolvedValue({
      ...downloading,
      state: "cancelled",
    });
    renderWorkspace();

    const reportItem = (await screen.findByText("report.txt")).closest("li");
    const notesItem = screen.getByText("notes.txt").closest("li");
    expect(reportItem).not.toBeNull();
    expect(notesItem).not.toBeNull();
    fireEvent.click(within(reportItem as HTMLElement).getByRole("button", { name: "保存" }));

    const cancel = await within(reportItem as HTMLElement).findByRole("button", {
      name: "已下载 50% · 取消",
    });
    expect(within(notesItem as HTMLElement).getByRole("button", { name: "保存" })).toBeTruthy();
    fireEvent.click(cancel);

    expect(await within(reportItem as HTMLElement).findByText("下载已取消")).toBeTruthy();
    expect(within(reportItem as HTMLElement).getByRole("button", { name: "保存" })).toBeTruthy();
  });

  it("附件失败时显示安全错误并允许独立重试", async () => {
    const readSummary = { ...summary, read: true, hasAttachments: true };
    vi.mocked(getInboxPage).mockResolvedValue({ items: [readSummary], nextCursor: null });
    vi.mocked(getMailMessageDetail).mockResolvedValue(
      attachmentDetail([{ id: attachmentId, fileName: "report.txt" }]),
    );
    vi.mocked(beginMailAttachmentDownload)
      .mockResolvedValueOnce({
        operationId,
        attachmentId,
        state: "failed",
        bytesWritten: "0",
        totalBytes: "12",
        error: {
          code: "destination_collision",
          message: "目标位置已有同名文件，请选择其他名称。",
          retryable: true,
        },
      })
      .mockResolvedValueOnce({
        operationId: retryOperationId,
        attachmentId,
        state: "completed",
        bytesWritten: "12",
        totalBytes: "12",
        error: null,
      });
    renderWorkspace();

    fireEvent.click(await screen.findByRole("button", { name: "保存" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "目标位置已有同名文件，请选择其他名称。",
    );
    fireEvent.click(screen.getByRole("button", { name: "重试" }));

    expect(await screen.findByRole("button", { name: "已保存" })).toBeTruthy();
    expect(beginMailAttachmentDownload).toHaveBeenCalledTimes(2);
  });
});
