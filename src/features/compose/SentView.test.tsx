import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  getSentItems,
  permitOutboundRetry,
  refreshLocalSentItems,
  type SentItemV1,
} from "../../lib/ipc/compose";
import { SentView } from "./SentView";

vi.mock("../../lib/ipc/compose", () => ({
  getSentItems: vi.fn(),
  permitOutboundRetry: vi.fn(),
  refreshLocalSentItems: vi.fn(),
}));

const accountId = "00000000-0000-4000-8000-000000000001";
const attemptId = "00000000-0000-4000-8000-000000000002";
const draftId = "00000000-0000-4000-8000-000000000003";
const account = {
  id: accountId,
  provider: "gmail" as const,
  email: "owner@example.test",
  displayName: null,
  authState: "connected" as const,
};
const pending: SentItemV1 = {
  attemptId,
  draftId,
  accountId,
  state: "accepted_pending",
  sender: { displayName: null, address: "owner@example.test" },
  to: [{ displayName: null, address: "recipient@example.test" }],
  cc: [],
  bcc: [],
  subject: "等待对账测试",
  plainBody: "本地发送内容",
  providerObserved: false,
  reconciledMessageId: null,
  canAuthorizeRetry: false,
  retryAuthorized: false,
  updatedAtMs: "42",
};

function renderSent(onOpenDraft = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <SentView accounts={[account]} onOpenDraft={onOpenDraft} />
    </QueryClientProvider>,
  );
}

describe("SentView", () => {
  beforeEach(() => {
    vi.mocked(getSentItems).mockReset();
    vi.mocked(permitOutboundRetry).mockReset();
    vi.mocked(refreshLocalSentItems).mockReset();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("把已接受但未观察到的邮件显示为等待邮箱确认", async () => {
    vi.mocked(getSentItems).mockResolvedValue([pending]);
    renderSent();

    expect((await screen.findAllByText("等待对账测试")).length).toBe(2);
    expect(screen.getAllByText("等待邮箱确认").length).toBeGreaterThan(0);
    expect(
      screen.getByText("邮箱已接受处理，正在等待远端已发送记录确认；这不是伪造的服务器邮件。"),
    ).toBeTruthy();
  });

  it("未知发送结果必须确认风险后才重新打开草稿", async () => {
    const onOpenDraft = vi.fn();
    vi.mocked(getSentItems).mockResolvedValue([
      { ...pending, state: "unknown_locked", canAuthorizeRetry: true },
    ]);
    vi.mocked(permitOutboundRetry).mockResolvedValue({ attemptId, authorized: true });
    vi.spyOn(window, "confirm").mockReturnValue(true);
    renderSent(onOpenDraft);

    fireEvent.click(await screen.findByRole("button", { name: "我已检查，仍要再次发送" }));
    await waitFor(() => expect(permitOutboundRetry).toHaveBeenCalledWith(attemptId));
    await waitFor(() => expect(onOpenDraft).toHaveBeenCalledWith(draftId));
  });

  it("手动刷新逐个查询已连接账户", async () => {
    vi.mocked(getSentItems).mockResolvedValue([]);
    vi.mocked(refreshLocalSentItems).mockResolvedValue({ accountId, updatedAttempts: 0 });
    renderSent();

    fireEvent.click(await screen.findByRole("button", { name: "刷新已发送" }));
    await waitFor(() => expect(refreshLocalSentItems).toHaveBeenCalledWith(accountId));
    expect(await screen.findByText("已检查本地已发送状态。")).toBeTruthy();
  });
});
