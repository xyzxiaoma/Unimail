import {
  assignMessageReadState,
  beginAttachmentDownload,
  cancelAttachmentDownload,
  fetchMessageRemoteImage,
  getAttachmentDownloadStatus,
  getMessageDetail,
  listInboxMessages,
  searchInboxMessages,
  type AddressRole,
  type AssignReadStateResultV1,
  type AttachmentDownloadCommandError,
  type AttachmentDownloadErrorCode,
  type AttachmentDownloadSnapshotV1,
  type AttachmentDownloadStateV1,
  type InboxMessageSummaryV1,
  type InboxPageRequestV1,
  type InboxPageV1,
  type MessageAddressV1,
  type MessageDetailV1,
  type MessageDirection,
  type ReaderAttachmentV1,
  type RemoteImageResultV1,
  type SearchPageRequestV1,
  type SearchPageV1,
} from "./bindings";
import { isNullableString, isRecord, isUint32, isUnsignedIntegerString, isUuid } from "./decode";
import { isRasterImageMediaType, isSafeRasterDataUrl } from "../security/raster-data-url";

export type {
  AssignReadStateResultV1,
  AttachmentDownloadCommandError,
  AttachmentDownloadSnapshotV1,
  InboxMessageSummaryV1,
  InboxPageRequestV1,
  InboxPageV1,
  MessageDetailV1,
  RemoteImageResultV1,
  SearchPageRequestV1,
  SearchPageV1,
} from "./bindings";

const addressRoles = new Set<AddressRole>(["from", "sender", "to", "cc", "bcc", "reply_to"]);
const messageDirections = new Set<MessageDirection>(["incoming", "outgoing"]);
const attachmentStates = new Set<AttachmentDownloadStateV1>([
  "downloading",
  "completed",
  "cancelled",
  "failed",
]);
const attachmentErrorContracts = {
  attachment_not_found: ["未找到这个附件。", false],
  attachment_unavailable: ["这个附件暂时无法下载。", false],
  account_unavailable: ["该邮箱账户当前不可用，请重新连接后重试。", false],
  offline: ["当前处于离线状态，无法下载附件。", true],
  destination_collision: ["目标位置已有同名文件，请选择其他名称。", true],
  attachment_too_large: ["附件超过当前允许的下载大小。", false],
  download_cancelled: ["附件下载已取消。", false],
  provider_failed: ["邮箱服务未能下载附件，请稍后重试。", true],
  write_failed: ["无法将附件写入所选位置。", true],
  verification_failed: ["附件完整性校验失败，请重新下载。", false],
  storage_unavailable: ["本地邮件存储暂时不可用。", true],
  internal: ["附件下载发生错误，请稍后重试。", true],
} as const satisfies Record<AttachmentDownloadErrorCode, readonly [string, boolean]>;

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

export function decodeSearchPage(value: unknown): SearchPageV1 {
  if (!isRecord(value) || !Array.isArray(value.items) || !isNullableString(value.nextCursor)) {
    throw new TypeError("搜索分页返回了无效数据");
  }
  return {
    items: value.items.map((item) => {
      if (!isRecord(item) || !isNullableString(item.matchContext)) {
        throw new TypeError("搜索结果包含无效字段");
      }
      return {
        summary: decodeMessageSummary(item.summary),
        matchContext: item.matchContext,
      };
    }),
    nextCursor: value.nextCursor,
  };
}

export function decodeAttachmentDownloadError(value: unknown): AttachmentDownloadCommandError {
  if (!isRecord(value)) {
    throw new TypeError("附件下载错误必须为对象");
  }
  const { code, message, retryable } = value;
  if (typeof code !== "string" || !(code in attachmentErrorContracts)) {
    throw new TypeError("附件下载错误代码无效");
  }
  const typedCode = code as AttachmentDownloadErrorCode;
  const contract = attachmentErrorContracts[typedCode];
  if (
    typeof message !== "string" ||
    typeof retryable !== "boolean" ||
    message !== contract[0] ||
    retryable !== contract[1]
  ) {
    throw new TypeError("附件下载错误契约无效");
  }
  return { code: typedCode, message, retryable };
}

export function decodeAttachmentDownloadSnapshot(value: unknown): AttachmentDownloadSnapshotV1 {
  if (!isRecord(value)) {
    throw new TypeError("附件下载状态必须为对象");
  }
  const { operationId, attachmentId, state, bytesWritten, totalBytes, error } = value;
  if (
    !isUuid(operationId) ||
    !isUuid(attachmentId) ||
    typeof state !== "string" ||
    !attachmentStates.has(state as AttachmentDownloadStateV1) ||
    !isUnsignedIntegerString(bytesWritten) ||
    !(totalBytes === null || isUnsignedIntegerString(totalBytes))
  ) {
    throw new TypeError("附件下载状态包含无效字段");
  }
  const typedState = state as AttachmentDownloadStateV1;
  const decodedError = error === null ? null : decodeAttachmentDownloadError(error);
  if ((typedState === "failed") !== (decodedError !== null)) {
    throw new TypeError("附件下载状态与错误不一致");
  }
  return {
    operationId,
    attachmentId,
    state: typedState,
    bytesWritten,
    totalBytes,
    error: decodedError,
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
  if (!isRasterImageMediaType(mediaType) || !isSafeRasterDataUrl(dataUrl, mediaType)) {
    throw new TypeError("远程图片返回了无效数据");
  }
  return { mediaType, dataUrl };
}

export async function getInboxPage(request: InboxPageRequestV1): Promise<InboxPageV1> {
  return decodeInboxPage(await listInboxMessages(request));
}

export async function getSearchPage(request: SearchPageRequestV1): Promise<SearchPageV1> {
  return decodeSearchPage(await searchInboxMessages(request));
}

export async function beginMailAttachmentDownload(
  attachmentId: string,
): Promise<AttachmentDownloadSnapshotV1 | null> {
  const value = await beginAttachmentDownload(attachmentId);
  return value === null ? null : decodeAttachmentDownloadSnapshot(value);
}

export async function getMailAttachmentDownloadStatus(
  operationId: string,
): Promise<AttachmentDownloadSnapshotV1> {
  return decodeAttachmentDownloadSnapshot(await getAttachmentDownloadStatus(operationId));
}

export async function cancelMailAttachmentDownload(
  operationId: string,
): Promise<AttachmentDownloadSnapshotV1> {
  return decodeAttachmentDownloadSnapshot(await cancelAttachmentDownload(operationId));
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
