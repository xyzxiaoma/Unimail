import { describe, expect, it } from "vitest";
import {
  decodeComposeCommandError,
  decodeDraft,
  decodeDraftList,
  decodeExplicitSendResult,
  decodeSentItems,
} from "./compose";

const draftId = "00000000-0000-4000-8000-000000000001";
const accountId = "00000000-0000-4000-8000-000000000002";
const attemptId = "00000000-0000-4000-8000-000000000003";
const messageId = "00000000-0000-4000-8000-000000000004";

const draft = {
  id: draftId,
  accountId,
  to: [{ displayName: "收件人", address: "recipient@example.test" }],
  cc: [],
  bcc: [],
  subject: "解码测试",
  plainBody: "仅使用虚构测试内容。",
  reply: false,
  revision: "1",
  createdAtMs: "10",
  updatedAtMs: "11",
  offlineReviewRequired: false,
};

const acceptedSentItem = {
  attemptId,
  draftId,
  accountId,
  state: "accepted_pending",
  sender: { displayName: null, address: "sender@example.test" },
  to: draft.to,
  cc: [],
  bcc: [],
  subject: draft.subject,
  plainBody: draft.plainBody,
  providerObserved: false,
  reconciledMessageId: null,
  canAuthorizeRetry: false,
  retryAuthorized: false,
  updatedAtMs: "12",
};

describe("compose IPC decoders", () => {
  it("接受完整草稿、草稿摘要、发送结果和已发送项目", () => {
    expect(decodeDraft(draft)).toEqual(draft);
    expect(
      decodeDraftList([
        {
          id: draftId,
          accountId,
          subject: draft.subject,
          recipientCount: 1,
          revision: "1",
          updatedAtMs: "11",
          offlineReviewRequired: false,
        },
      ]),
    ).toHaveLength(1);
    expect(
      decodeExplicitSendResult({
        state: "accepted_pending",
        draft: null,
        attemptId,
        errorCode: null,
      }),
    ).toEqual({ state: "accepted_pending", draft: null, attemptId, errorCode: null });
    expect(decodeSentItems([acceptedSentItem])).toEqual([acceptedSentItem]);
  });

  it.each([
    ["malformed UUID", { ...draft, id: "not-a-uuid" }],
    ["zero revision", { ...draft, revision: "0" }],
    ["fractional revision", { ...draft, revision: "1.5" }],
  ])("拒绝 %s", (_label, payload) => {
    expect(() => decodeDraft(payload)).toThrow(TypeError);
  });

  it.each([
    {
      state: "offline_saved",
      draft: null,
      attemptId: null,
      errorCode: null,
    },
    {
      state: "accepted_pending",
      draft,
      attemptId,
      errorCode: null,
    },
    {
      state: "rejected",
      draft: null,
      attemptId,
      errorCode: null,
    },
    {
      state: "unknown_state",
      draft: null,
      attemptId,
      errorCode: null,
    },
  ])("拒绝无效发送状态组合 %#", (payload) => {
    expect(() => decodeExplicitSendResult(payload)).toThrow(TypeError);
  });

  it.each([
    { ...acceptedSentItem, state: "submitting" },
    { ...acceptedSentItem, providerObserved: true },
    {
      ...acceptedSentItem,
      state: "reconciled",
      providerObserved: true,
      reconciledMessageId: null,
    },
    {
      ...acceptedSentItem,
      state: "unknown_locked",
      canAuthorizeRetry: true,
      retryAuthorized: true,
    },
  ])("拒绝无效已发送状态组合 %#", (payload) => {
    expect(() => decodeSentItems([payload])).toThrow(TypeError);
  });

  it("只接受固定且完全匹配的安全错误合同", () => {
    expect(
      decodeComposeCommandError({
        code: "revision_conflict",
        message: "草稿已在其他位置更新，请重新打开后继续编辑。",
        retryable: true,
      }),
    ).toEqual({
      code: "revision_conflict",
      message: "草稿已在其他位置更新，请重新打开后继续编辑。",
      retryable: true,
    });
    expect(() =>
      decodeComposeCommandError({
        code: "revision_conflict",
        message: "C:\\private\\mail.db",
        retryable: true,
      }),
    ).toThrow(TypeError);
    expect(() =>
      decodeComposeCommandError({ code: "provider_secret", message: "泄漏", retryable: false }),
    ).toThrow(TypeError);
  });

  it("接受远端对账完成的唯一合法组合", () => {
    expect(
      decodeSentItems([
        {
          ...acceptedSentItem,
          state: "reconciled",
          providerObserved: true,
          reconciledMessageId: messageId,
        },
      ])[0]?.state,
    ).toBe("reconciled");
  });
});
