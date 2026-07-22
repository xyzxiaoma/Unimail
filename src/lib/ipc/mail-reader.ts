import {
  assignMessageReadState,
  fetchMessageRemoteImage,
  getMessageDetail,
  listInboxMessages,
  type AddressRole,
  type AssignReadStateResultV1,
  type InboxMessageSummaryV1,
  type InboxPageRequestV1,
  type InboxPageV1,
  type MessageAddressV1,
  type MessageDetailV1,
  type MessageDirection,
  type ReaderAttachmentV1,
  type RemoteImageResultV1,
} from "./bindings";
import { isRecord } from "./decode";

export type {
  AssignReadStateResultV1,
  InboxMessageSummaryV1,
  InboxPageRequestV1,
  InboxPageV1,
  MessageDetailV1,
  RemoteImageResultV1,
} from "./bindings";

const addressRoles = new Set<AddressRole>(["from", "sender", "to", "cc", "bcc", "reply_to"]);
const messageDirections = new Set<MessageDirection>(["incoming", "outgoing"]);
const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu;
const unsignedIntegerPattern = /^(0|[1-9][0-9]*)$/u;
const remoteImageDataUrlPattern = /^data:image\/(png|jpeg|gif|webp);base64,[A-Za-z0-9+/]+={0,2}$/u;
const remoteImageMediaTypes = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const maxRemoteImageDataUrlLength = 2_800_000;

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}

function isUuid(value: unknown): value is string {
  return typeof value === "string" && uuidPattern.test(value);
}

function isUnsignedIntegerString(value: unknown): value is string {
  return typeof value === "string" && unsignedIntegerPattern.test(value);
}

function isUint32(value: unknown): value is number {
  return (
    typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= 4_294_967_295
  );
}

function decodeMessageSummary(value: unknown): InboxMessageSummaryV1 {
  if (!isRecord(value)) {
    throw new TypeError("邮件摘要必须为对象");
  }
  const {
    id,
    accountId,
    mailboxId,
    subject,
    snippet,
    senderName,
    senderAddress,
    read,
    direction,
    sentAtMs,
    receivedAtMs,
    hasAttachments,
  } = value;
  if (
    !isUuid(id) ||
    !isUuid(accountId) ||
    !isUuid(mailboxId) ||
    !isNullableString(subject) ||
    !isNullableString(snippet) ||
    !isNullableString(senderName) ||
    !isNullableString(senderAddress) ||
    typeof read !== "boolean" ||
    typeof direction !== "string" ||
    !messageDirections.has(direction as MessageDirection) ||
    !(sentAtMs === null || isUnsignedIntegerString(sentAtMs)) ||
    !isUnsignedIntegerString(receivedAtMs) ||
    typeof hasAttachments !== "boolean"
  ) {
    throw new TypeError("邮件摘要包含无效字段");
  }
  return {
    id,
    accountId,
    mailboxId,
    subject,
    snippet,
    senderName,
    senderAddress,
    read,
    direction: direction as MessageDirection,
    sentAtMs,
    receivedAtMs,
    hasAttachments,
  };
}

function decodeAddress(value: unknown): MessageAddressV1 {
  if (!isRecord(value)) {
    throw new TypeError("邮件地址必须为对象");
  }
  const { role, position, displayName, address } = value;
  if (
    typeof role !== "string" ||
    !addressRoles.has(role as AddressRole) ||
    !isUint32(position) ||
    !isNullableString(displayName) ||
    typeof address !== "string" ||
    address.length === 0
  ) {
    throw new TypeError("邮件地址包含无效字段");
  }
  return { role: role as AddressRole, position, displayName, address };
}

function decodeAttachment(value: unknown): ReaderAttachmentV1 {
  if (!isRecord(value)) {
    throw new TypeError("附件摘要必须为对象");
  }
  const { id, fileName, mediaType, sizeBytes, contentId, inline } = value;
  if (
    !isUuid(id) ||
    !isNullableString(fileName) ||
    typeof mediaType !== "string" ||
    mediaType.length === 0 ||
    !(sizeBytes === null || isUnsignedIntegerString(sizeBytes)) ||
    !isNullableString(contentId) ||
    typeof inline !== "boolean"
  ) {
    throw new TypeError("附件摘要包含无效字段");
  }
  return { id, fileName, mediaType, sizeBytes, contentId, inline };
}

export function decodeInboxPage(value: unknown): InboxPageV1 {
  if (!isRecord(value) || !Array.isArray(value.items) || !isNullableString(value.nextCursor)) {
    throw new TypeError("收件箱分页返回了无效数据");
  }
  return {
    items: value.items.map(decodeMessageSummary),
    nextCursor: value.nextCursor,
  };
}

export function decodeMessageDetail(value: unknown): MessageDetailV1 {
  if (!isRecord(value)) {
    throw new TypeError("邮件详情必须为对象");
  }
  const {
    summary,
    threadId,
    rfcMessageId,
    plainBody,
    htmlBody,
    parserVersion,
    sanitizerVersion,
    addresses,
    attachments,
  } = value;
  if (
    !isNullableString(threadId) ||
    !isNullableString(rfcMessageId) ||
    !isNullableString(plainBody) ||
    !isNullableString(htmlBody) ||
    !isUint32(parserVersion) ||
    parserVersion === 0 ||
    !isUint32(sanitizerVersion) ||
    sanitizerVersion === 0 ||
    !Array.isArray(addresses) ||
    !Array.isArray(attachments)
  ) {
    throw new TypeError("邮件详情包含无效字段");
  }
  return {
    summary: decodeMessageSummary(summary),
    threadId,
    rfcMessageId,
    plainBody,
    htmlBody,
    parserVersion,
    sanitizerVersion,
    addresses: addresses.map(decodeAddress),
    attachments: attachments.map(decodeAttachment),
  };
}

export function decodeAssignReadStateResult(value: unknown): AssignReadStateResultV1 {
  if (!isRecord(value)) {
    throw new TypeError("已读状态返回必须为对象");
  }
  const { messageId, read, generation } = value;
  if (!isUuid(messageId) || typeof read !== "boolean" || !isUnsignedIntegerString(generation)) {
    throw new TypeError("已读状态返回了无效数据");
  }
  return { messageId, read, generation };
}

export function decodeRemoteImageResult(value: unknown): RemoteImageResultV1 {
  if (!isRecord(value)) {
    throw new TypeError("远程图片返回必须为对象");
  }
  const { mediaType, dataUrl } = value;
  if (
    typeof mediaType !== "string" ||
    !remoteImageMediaTypes.has(mediaType) ||
    typeof dataUrl !== "string" ||
    dataUrl.length > maxRemoteImageDataUrlLength ||
    !remoteImageDataUrlPattern.test(dataUrl) ||
    !dataUrl.startsWith(`data:${mediaType};base64,`)
  ) {
    throw new TypeError("远程图片返回了无效数据");
  }
  return { mediaType, dataUrl };
}

export async function getInboxPage(request: InboxPageRequestV1): Promise<InboxPageV1> {
  return decodeInboxPage(await listInboxMessages(request));
}

export async function getMailMessageDetail(messageId: string): Promise<MessageDetailV1> {
  return decodeMessageDetail(await getMessageDetail(messageId));
}

export async function setMailMessageRead(
  messageId: string,
  read: boolean,
): Promise<AssignReadStateResultV1> {
  return decodeAssignReadStateResult(await assignMessageReadState(messageId, read));
}

export async function getMailRemoteImage(
  messageId: string,
  url: string,
): Promise<RemoteImageResultV1> {
  return decodeRemoteImageResult(await fetchMessageRemoteImage(messageId, url));
}
