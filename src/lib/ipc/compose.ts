import {
  authorizeOutboundRetry,
  createReplyDraft,
  deleteDraft,
  getDraft,
  listDrafts,
  listSentItems,
  refreshSentItems,
  reportConnectivity,
  saveDraft,
  sendDraft,
  type ComposeCommandError,
  type ComposeCommandErrorCode,
  type DraftAddressV1,
  type DraftSummaryV1,
  type DraftV1,
  type ExplicitSendRequestV1,
  type ExplicitSendResultV1,
  type ExplicitSendStateV1,
  type OutboundAttemptState,
  type OutboundFailureCode,
  type RetryAuthorizationResultV1,
  type SaveDraftRequestV1,
  type SentItemV1,
  type SentRefreshResultV1,
} from "./bindings";
import { isNullableString, isRecord, isUint32, isUnsignedIntegerString, isUuid } from "./decode";

export type {
  ComposeCommandError,
  DraftAddressV1,
  DraftSummaryV1,
  DraftV1,
  ExplicitSendRequestV1,
  ExplicitSendResultV1,
  SaveDraftRequestV1,
  SentItemV1,
} from "./bindings";

const composeErrorContracts: Record<
  ComposeCommandErrorCode,
  Pick<ComposeCommandError, "message" | "retryable">
> = {
  invalid_data: { message: "草稿内容格式无效，请检查后重试。", retryable: false },
  not_found: { message: "未找到这封本地草稿。", retryable: false },
  revision_conflict: {
    message: "草稿已在其他位置更新，请重新打开后继续编辑。",
    retryable: true,
  },
  account_unavailable: {
    message: "发件账户当前不可用，请重新连接或选择其他账户。",
    retryable: true,
  },
  empty_subject_confirmation_required: {
    message: "请确认是否发送没有主题的邮件。",
    retryable: false,
  },
  offline_review_confirmation_required: {
    message: "请联网后重新检查草稿，并再次确认发送。",
    retryable: false,
  },
  send_locked: { message: "发送结果可能已提交，请先检查已发送邮件。", retryable: false },
  storage_unavailable: { message: "无法访问本地加密草稿，请稍后重试。", retryable: true },
  internal: { message: "写信功能暂时不可用，请稍后重试。", retryable: true },
};

const composeErrorCodes = new Set<ComposeCommandErrorCode>(
  Object.keys(composeErrorContracts) as ComposeCommandErrorCode[],
);
const explicitSendStates = new Set<ExplicitSendStateV1>([
  "offline_saved",
  "accepted_pending",
  "rejected",
  "unknown_locked",
]);
const outboundAttemptStates = new Set<OutboundAttemptState>([
  "submitting",
  "accepted_pending",
  "reconciled",
  "rejected",
  "unknown_locked",
]);
const outboundFailureCodes = new Set<OutboundFailureCode>([
  "recipient_rejected",
  "authentication_required",
  "provider_unavailable",
  "invalid_draft",
  "internal",
]);

function decodeDraftAddress(value: unknown): DraftAddressV1 {
  if (!isRecord(value)) throw new TypeError("草稿地址必须为对象");
  const { displayName, address } = value;
  if (!isNullableString(displayName) || typeof address !== "string") {
    throw new TypeError("草稿地址包含无效字段");
  }
  return { displayName, address };
}

function decodeDraftAddressList(value: unknown): DraftAddressV1[] {
  if (!Array.isArray(value)) throw new TypeError("草稿地址列表必须为数组");
  return value.map(decodeDraftAddress);
}

export function decodeDraft(value: unknown): DraftV1 {
  if (!isRecord(value)) throw new TypeError("草稿必须为对象");
  const {
    id,
    accountId,
    to,
    cc,
    bcc,
    subject,
    plainBody,
    reply,
    revision,
    createdAtMs,
    updatedAtMs,
    offlineReviewRequired,
  } = value;
  if (
    !isUuid(id) ||
    !isUuid(accountId) ||
    typeof subject !== "string" ||
    typeof plainBody !== "string" ||
    typeof reply !== "boolean" ||
    !isUnsignedIntegerString(revision) ||
    revision === "0" ||
    !isUnsignedIntegerString(createdAtMs) ||
    !isUnsignedIntegerString(updatedAtMs) ||
    typeof offlineReviewRequired !== "boolean"
  ) {
    throw new TypeError("草稿包含无效字段");
  }
  return {
    id,
    accountId,
    to: decodeDraftAddressList(to),
    cc: decodeDraftAddressList(cc),
    bcc: decodeDraftAddressList(bcc),
    subject,
    plainBody,
    reply,
    revision,
    createdAtMs,
    updatedAtMs,
    offlineReviewRequired,
  };
}

function decodeDraftSummary(value: unknown): DraftSummaryV1 {
  if (!isRecord(value)) throw new TypeError("草稿摘要必须为对象");
  const { id, accountId, subject, recipientCount, revision, updatedAtMs, offlineReviewRequired } =
    value;
  if (
    !isUuid(id) ||
    !isUuid(accountId) ||
    typeof subject !== "string" ||
    !isUint32(recipientCount) ||
    !isUnsignedIntegerString(revision) ||
    revision === "0" ||
    !isUnsignedIntegerString(updatedAtMs) ||
    typeof offlineReviewRequired !== "boolean"
  ) {
    throw new TypeError("草稿摘要包含无效字段");
  }
  return { id, accountId, subject, recipientCount, revision, updatedAtMs, offlineReviewRequired };
}

export function decodeDraftList(value: unknown): DraftSummaryV1[] {
  if (!Array.isArray(value)) throw new TypeError("草稿列表必须为数组");
  return value.map(decodeDraftSummary);
}

export function decodeExplicitSendResult(value: unknown): ExplicitSendResultV1 {
  if (!isRecord(value)) throw new TypeError("发送结果必须为对象");
  const { state, draft, attemptId, errorCode } = value;
  if (
    typeof state !== "string" ||
    !explicitSendStates.has(state as ExplicitSendStateV1) ||
    !(draft === null || isRecord(draft)) ||
    !(attemptId === null || isUuid(attemptId)) ||
    !(
      errorCode === null ||
      (typeof errorCode === "string" && outboundFailureCodes.has(errorCode as OutboundFailureCode))
    )
  ) {
    throw new TypeError("发送结果包含无效字段");
  }
  const decodedDraft = draft === null ? null : decodeDraft(draft);
  if (
    (state === "offline_saved" && (decodedDraft === null || attemptId !== null)) ||
    (state !== "offline_saved" && (decodedDraft !== null || attemptId === null)) ||
    (state === "rejected" && errorCode === null) ||
    (state !== "rejected" && errorCode !== null)
  ) {
    throw new TypeError("发送结果状态组合无效");
  }
  return {
    state: state as ExplicitSendStateV1,
    draft: decodedDraft,
    attemptId,
    errorCode: errorCode as OutboundFailureCode | null,
  };
}

function decodeSentItem(value: unknown): SentItemV1 {
  if (!isRecord(value)) throw new TypeError("已发送项目必须为对象");
  const {
    attemptId,
    draftId,
    accountId,
    state,
    sender,
    to,
    cc,
    bcc,
    subject,
    plainBody,
    providerObserved,
    reconciledMessageId,
    canAuthorizeRetry,
    retryAuthorized,
    updatedAtMs,
  } = value;
  if (
    !isUuid(attemptId) ||
    !isUuid(draftId) ||
    !isUuid(accountId) ||
    typeof state !== "string" ||
    !outboundAttemptStates.has(state as OutboundAttemptState) ||
    typeof subject !== "string" ||
    typeof plainBody !== "string" ||
    typeof providerObserved !== "boolean" ||
    !(reconciledMessageId === null || isUuid(reconciledMessageId)) ||
    typeof canAuthorizeRetry !== "boolean" ||
    typeof retryAuthorized !== "boolean" ||
    !isUnsignedIntegerString(updatedAtMs)
  ) {
    throw new TypeError("已发送项目包含无效字段");
  }
  const sentState = state as OutboundAttemptState;
  const validStateCombination =
    (sentState === "accepted_pending" &&
      !providerObserved &&
      reconciledMessageId === null &&
      !canAuthorizeRetry &&
      !retryAuthorized) ||
    (sentState === "reconciled" &&
      providerObserved &&
      reconciledMessageId !== null &&
      !canAuthorizeRetry &&
      !retryAuthorized) ||
    (sentState === "unknown_locked" &&
      !providerObserved &&
      reconciledMessageId === null &&
      !(canAuthorizeRetry && retryAuthorized));
  if (!validStateCombination) {
    throw new TypeError("已发送项目状态组合无效");
  }
  return {
    attemptId,
    draftId,
    accountId,
    state: sentState,
    sender: decodeDraftAddress(sender),
    to: decodeDraftAddressList(to),
    cc: decodeDraftAddressList(cc),
    bcc: decodeDraftAddressList(bcc),
    subject,
    plainBody,
    providerObserved,
    reconciledMessageId,
    canAuthorizeRetry,
    retryAuthorized,
    updatedAtMs,
  };
}

export function decodeSentItems(value: unknown): SentItemV1[] {
  if (!Array.isArray(value)) throw new TypeError("已发送列表必须为数组");
  return value.map(decodeSentItem);
}

export function decodeComposeCommandError(value: unknown): ComposeCommandError {
  if (!isRecord(value)) throw new TypeError("写信命令错误必须为对象");
  const { code, message, retryable } = value;
  if (
    typeof code !== "string" ||
    !composeErrorCodes.has(code as ComposeCommandErrorCode) ||
    typeof message !== "string" ||
    typeof retryable !== "boolean"
  ) {
    throw new TypeError("写信命令返回了无效错误");
  }
  const contract = composeErrorContracts[code as ComposeCommandErrorCode];
  if (message !== contract.message || retryable !== contract.retryable) {
    throw new TypeError("写信命令错误与固定合同不一致");
  }
  return { code: code as ComposeCommandErrorCode, message, retryable };
}

function decodeSentRefreshResult(value: unknown): SentRefreshResultV1 {
  if (!isRecord(value) || !isUuid(value.accountId) || !isUint32(value.updatedAttempts)) {
    throw new TypeError("已发送刷新结果无效");
  }
  return { accountId: value.accountId, updatedAttempts: value.updatedAttempts };
}

function decodeRetryAuthorizationResult(value: unknown): RetryAuthorizationResultV1 {
  if (!isRecord(value) || !isUuid(value.attemptId) || typeof value.authorized !== "boolean") {
    throw new TypeError("再次发送授权结果无效");
  }
  return { attemptId: value.attemptId, authorized: value.authorized };
}

export async function getDrafts(accountId: string | null = null): Promise<DraftSummaryV1[]> {
  return decodeDraftList(await listDrafts(accountId));
}

export async function getLocalDraft(draftId: string): Promise<DraftV1> {
  return decodeDraft(await getDraft(draftId));
}

export async function saveLocalDraft(request: SaveDraftRequestV1): Promise<DraftV1> {
  return decodeDraft(await saveDraft(request));
}

export async function removeLocalDraft(draftId: string): Promise<boolean> {
  const value = await deleteDraft(draftId);
  if (typeof value !== "boolean") throw new TypeError("删除草稿结果无效");
  return value;
}

export async function createLocalReplyDraft(messageId: string): Promise<DraftV1> {
  return decodeDraft(await createReplyDraft(messageId));
}

export async function submitLocalDraft(
  request: ExplicitSendRequestV1,
): Promise<ExplicitSendResultV1> {
  return decodeExplicitSendResult(await sendDraft(request));
}

export async function getSentItems(accountId: string | null = null): Promise<SentItemV1[]> {
  return decodeSentItems(await listSentItems(accountId));
}

export async function refreshLocalSentItems(accountId: string): Promise<SentRefreshResultV1> {
  return decodeSentRefreshResult(await refreshSentItems(accountId));
}

export async function permitOutboundRetry(attemptId: string): Promise<RetryAuthorizationResultV1> {
  return decodeRetryAuthorizationResult(await authorizeOutboundRetry(attemptId));
}

export async function reportDesktopConnectivity(online: boolean): Promise<void> {
  await reportConnectivity(online);
}
