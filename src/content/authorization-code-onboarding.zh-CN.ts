import type { AuthorizationCodeProvider } from "../lib/ipc/authorization-code-onboarding";

export const authorizationCodeCopy = {
  qq: {
    providerName: "QQ 邮箱",
    title: "连接 QQ 邮箱",
    domain: "qq.com",
    addressPlaceholder: "你的账号@qq.com",
    guidance: "请先在 QQ 邮箱设置中启用 IMAP/SMTP 服务，并生成授权码。这里不能填写 QQ 密码。",
  },
  netease: {
    providerName: "163 邮箱",
    title: "连接 163 邮箱",
    domain: "163.com",
    addressPlaceholder: "你的账号@163.com",
    guidance:
      "请先在 163 邮箱设置中启用 IMAP/SMTP 服务，并生成客户端授权码。这里不能填写邮箱密码。",
  },
} as const satisfies Record<AuthorizationCodeProvider, object>;
