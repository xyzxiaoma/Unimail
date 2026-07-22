import { describe, expect, it } from "vitest";
import {
  decodeAssignReadStateResult,
  decodeInboxPage,
  decodeMessageDetail,
  decodeRemoteImageResult,
} from "./mail-reader";

const messageId = "00000000-0000-4000-8000-000000000001";
const accountId = "00000000-0000-4000-8000-000000000002";
const mailboxId = "00000000-0000-4000-8000-000000000003";

const summary = {
  id: messageId,
  accountId,
  mailboxId,
  subject: "项目进展",
  snippet: "这是一条虚构摘要",
  senderName: "测试发件人",
  senderAddress: "sender@example.test",
  read: false,
  direction: "incoming",
  sentAtMs: null,
  receivedAtMs: "42",
  hasAttachments: true,
};

describe("mail reader IPC decoders", () => {
  it("accepts complete Inbox and detail payloads", () => {
    expect(decodeInboxPage({ items: [summary], nextCursor: "v1:42:cursor" })).toEqual({
      items: [summary],
      nextCursor: "v1:42:cursor",
    });
    expect(
      decodeMessageDetail({
        summary,
        threadId: null,
        rfcMessageId: "fictional@example.test",
        plainBody: "虚构正文",
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
        attachments: [
          {
            id: "00000000-0000-4000-8000-000000000004",
            fileName: "report.txt",
            mediaType: "text/plain",
            sizeBytes: "12",
            contentId: null,
            inline: false,
          },
        ],
      }).summary,
    ).toEqual(summary);
  });

  it.each([
    null,
    { items: {}, nextCursor: null },
    { items: [{ ...summary, id: "bad" }], nextCursor: null },
    { items: [{ ...summary, receivedAtMs: 42 }], nextCursor: null },
    { items: [{ ...summary, direction: "unknown" }], nextCursor: null },
  ])("rejects malformed Inbox payload %#", (payload) => {
    expect(() => decodeInboxPage(payload)).toThrow(TypeError);
  });

  it("validates read-state generations", () => {
    expect(decodeAssignReadStateResult({ messageId, read: true, generation: "7" })).toEqual({
      messageId,
      read: true,
      generation: "7",
    });
    expect(() => decodeAssignReadStateResult({ messageId, read: true, generation: -1 })).toThrow(
      TypeError,
    );
  });

  it("只接受受限图片类型和本地 data URL", () => {
    const payload = { mediaType: "image/png", dataUrl: "data:image/png;base64,iVBORw0KGgo=" };
    expect(decodeRemoteImageResult(payload)).toEqual(payload);
    for (const invalid of [
      { mediaType: "image/svg+xml", dataUrl: "data:image/svg+xml;base64,PHN2Zz4=" },
      { mediaType: "image/png", dataUrl: "https://images.example.test/a.png" },
      { mediaType: "image/jpeg", dataUrl: "data:image/png;base64,iVBORw0KGgo=" },
    ]) {
      expect(() => decodeRemoteImageResult(invalid)).toThrow(TypeError);
    }
  });
});
