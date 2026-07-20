# Gmail 所有者验收清单

本清单用于仓库所有者在自己的 Google OAuth 应用和测试邮箱上验证 Gmail V1。自动化测试只使用本机虚构 HTTP/MIME fixtures，不包含真实账号、令牌或邮件。

## 准备

1. 在 Google Cloud Console 创建 `Desktop app` OAuth client，并完成适合测试账号的 consent screen 配置。
2. 确保授权范围包含：
   - `https://www.googleapis.com/auth/gmail.modify`
   - `https://www.googleapis.com/auth/gmail.send`
3. 在同一终端设置 `UNIMAIL_GMAIL_CLIENT_ID`，然后运行 `npm run tauri dev` 或构建测试安装包。
4. 使用专门的测试邮箱；不要把 client ID 以外的 OAuth 数据写进仓库文件，也不要截图或粘贴授权 URL、回调 URL、code、access token、refresh token。

## 连接与凭据

- [ ] 点击“添加邮箱账户”，选择 Gmail 后只打开系统浏览器，不在应用内嵌登录页。
- [ ] Google 授权完成后，浏览器显示静态中文完成页，Unimail 显示已连接账号。
- [ ] 取消授权、关闭流程、等待超时后，应用显示安全中文提示并允许重新尝试。
- [ ] 连续启动两个连接流程时，旧流程失效，旧页面不能覆盖新流程状态。
- [ ] 使用同一 Gmail 地址再次连接时更新原账户，不新增重复账户。
- [ ] 重启应用后账户仍存在，且无需再次输入令牌；SQLite 中不存在 access/refresh token。
- [ ] 在 Google 账号页撤销 Unimail 权限后触发同步，账户进入需要重新连接的状态，不无限自动重试。

## 同步与一致性

- [ ] 初次同步只导入 Inbox，按最新优先，最多 500 封。
- [ ] 初次同步过程中向邮箱投递新邮件，后续增量同步可以补回，不永久遗漏。
- [ ] 新邮件、外部删除/移出 Inbox、外部已读/未读变化经 Gmail History 增量同步后正确收敛。
- [ ] 多次手动同步不产生重复邮件，本地邮件 ID 保持稳定。
- [ ] 模拟或等待旧 History ID 失效后，只执行一次最新 500 封的有界重建，草稿、凭据及其他账号数据不被清空。
- [ ] 同时连接其他 Provider 后，Gmail worker 不处理其他 Provider 的同步任务。

## 阅读、附件与已读

- [ ] 含纯文本、HTML、multipart/alternative、中文编码、回复引用、CID 内联图片的邮件可正确解析。
- [ ] 附件列表显示正确名称/类型/大小；下载普通附件和内联附件得到正确内容。
- [ ] 取消附件下载或目标写入失败时不会留下成功状态，也不会在错误中出现本地路径。
- [ ] 在 Unimail 标记已读会移除 Gmail `UNREAD`；标记未读会添加 `UNREAD`。
- [ ] 重复执行同一个已读值保持幂等，不发生反向切换。

## 发送与回复边界

- [ ] 新邮件发送成功后由 Gmail 放入 Sent，返回的 Gmail message ID 与 RFC Message-ID 可用于对账。
- [ ] 回复同时保留 `In-Reply-To`/`References` 并传递 Gmail `threadId`，在 Gmail 中进入正确会话。
- [ ] 含附件的 MIME 原文未被 Gmail adapter 二次重组或修改。
- [ ] 断网发生在提交结果不明确时，Unimail 不自动重发；用户需要先核对 Sent，再决定后续操作。
- [ ] 离线点击发送仍只保留草稿并要求联网后重新确认，不因 Gmail adapter 存在而自动投递。

## 安全与诊断

- [ ] 普通 UI、状态栏、错误提示和终端输出中不出现授权 URL、回调 query、code、token、邮件正文、附件内容或本地数据库路径。
- [ ] Windows Credential Manager/macOS Keychain 中存在应用作用域的 Gmail 凭据项；删除/重连流程不会把令牌复制到 SQLite。
- [ ] 未设置 `UNIMAIL_GMAIL_CLIENT_ID` 的构建仍能启动，并显示“当前构建未配置 Gmail 接入”。
- [ ] 普通 push 的 GitHub Actions 仍只上传 Windows/macOS 测试安装包，不创建 GitHub Release。

验收问题请只记录固定错误码、平台、应用版本、操作阶段和可公开的 request ID；不要提交账号地址、邮件内容、OAuth 值或完整 Provider 响应。
