# Outlook 所有者验收清单

本清单用于仓库所有者在自己的 Microsoft Entra 应用和测试邮箱上验证 Outlook V1。自动化测试只使用本机虚构 HTTP/MIME 数据，不包含真实账号、令牌、delta 链接或邮件。

## Microsoft Entra 准备

1. 创建应用注册，支持“任何组织目录中的账户和个人 Microsoft 账户”。
2. 在“身份验证”中添加“移动和桌面应用程序”，配置 localhost 回调并启用公共客户端流；不要创建或配置 client secret。
3. 添加委托权限 `User.Read`、`Mail.ReadWrite`、`Mail.Send`。Unimail 登录时还请求 `offline_access`。
4. 设置 `UNIMAIL_OUTLOOK_CLIENT_ID` 后运行 `npm run tauri dev` 或构建测试安装包。
5. 使用专门的测试邮箱；不要记录授权 URL、回调 URL、code、access token、refresh token、完整 request URL 或 delta 链接。

## 连接与账户生命周期

- [ ] “添加邮箱账户”中可选择 Outlook，系统浏览器允许个人账户和工作/学校账户登录。
- [ ] 回调地址显示为动态端口 `localhost`，应用只接受本机 IPv4 回环连接、正确 Host、路径和 state。
- [ ] 完成授权后显示账户；取消、超时、租户策略或用户同意失败只显示固定中文错误。
- [ ] 同一标准化邮箱地址再次连接时更新原账户，不创建重复账户。
- [ ] 重启后账户和同步注册恢复；SQLite 不包含 access/refresh token、tenant ID 或 OAuth 回调值。
- [ ] 撤销授权或令牌失效后，账户进入“需要重新连接”，不会无限自动重试。

## 同步与 delta

- [ ] 初次同步只导入 Inbox 最新不超过 500 封邮件。
- [ ] 初次同步期间投递或修改邮件，完成后再同步能够补回变化，不出现永久缺口。
- [ ] 新邮件、更新、已读/未读变化和移出 Inbox 的邮件通过 delta 正确收敛。
- [ ] 多页、空页、重复或乱序 delta 项不会产生重复本地邮件；Graph immutable ID 保持稳定。
- [ ] delta 状态过期或返回 `syncStateNotFound` 时只触发一次最新 500 封有界重建。
- [ ] Outlook worker 不会领取 Gmail、QQ 或 163 的同步任务。

## MIME、附件与已读

- [ ] 纯文本、HTML、回复引用、中文编码、CID 内联图片和 `message/rfc822` 可通过共享 MIME 解析器读取。
- [ ] 附件名称、类型、大小、CID 和内联状态与 Graph immutable attachment ID 对齐。
- [ ] 文件附件和项目附件可通过 `$value` 流式下载；取消、目标写入失败和大小超限不会产生成功状态。
- [ ] reference attachment 返回“暂不支持”，不会跟随或下载其云端 URL。
- [ ] 重复设置同一个已读值保持幂等，并返回 Graph 观察到的状态和修订。

## 发送、回复与对账

- [ ] 新邮件和回复使用共享组合器产生的同一份 MIME 字节；Graph 请求体是标准 Base64，不是 Base64URL。
- [ ] 回复使用原始 immutable message ID，保留 `Message-ID`、`In-Reply-To` 和 `References`，并进入正确会话。
- [ ] Graph `202 Accepted` 显示为已接受处理，不伪造 provider message ID；后续可通过稳定 RFC Message-ID 在 Sent 中对账。
- [ ] 提交后网络结果不明确时不自动重发，先检查 Sent 再决定是否重试。
- [ ] 离线发送仍只保留草稿并要求联网后重新确认。

## 安全与诊断

- [ ] UI、错误、终端输出和测试快照不出现账户地址、授权值、完整 Graph URL、delta 链接、附件 ID、邮件正文或本地路径。
- [ ] 仅允许记录格式受限的 Graph `request-id`；AAD/Graph 原始响应体不会跨 IPC。
- [ ] 未设置 `UNIMAIL_OUTLOOK_CLIENT_ID` 的构建仍能启动，Outlook 入口显示未配置，Gmail 与本地功能正常。
- [ ] 普通 push 只上传 Windows/macOS 未签名测试安装包，不创建 GitHub Release。

验收问题只记录固定错误码、平台、应用版本、操作阶段和可公开的 request ID。租户同意策略、发布者验证和生产邮箱配置属于所有者环境，自动化测试通过不代表这些外部条件已经完成。
