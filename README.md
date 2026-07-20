# Unimail

Unimail 是一个以简体中文为首发界面语言的跨平台桌面邮件客户端。本仓库正在按 V1 规格逐项实现；Windows 和 macOS 安装包仅用于测试，不代表已经完成代码签名、公证或自动更新集成。

## 环境要求

- Node.js 22 与 npm
- Rust 1.95 或更高版本，并安装 `rustfmt`、`clippy`
- Tauri 2 对应平台的系统依赖
- Windows 本地开发需要 Microsoft C++ Build Tools、Windows SDK 和 WebView2；请按 [Tauri Windows prerequisites](https://v2.tauri.app/start/prerequisites/#windows) 配置
- Windows 首次编译内置 SQLCipher/OpenSSL 还需要 Perl；推荐安装 Strawberry Perl，并确保 `perl` 在 `PATH` 中

安装依赖：

```powershell
npm ci
```

启动浏览器中的前端开发服务器：

```powershell
npm run dev
```

启动 Windows 桌面开发窗口：

```powershell
npm run tauri dev
```

## Gmail 接入配置

Gmail 使用 Google “桌面应用”类型的 OAuth 公开客户端、系统浏览器、PKCE 和本机回环回调。应用只需要 OAuth client ID，绝不需要或读取 client secret。

1. 在 Google Cloud Console 配置 OAuth consent screen，并创建类型为 `Desktop app` 的 OAuth client。
2. 在启动或构建 Unimail 的同一终端设置公开 client ID：

```powershell
$env:UNIMAIL_GMAIL_CLIENT_ID="你的公开桌面客户端ID"
npm run tauri dev
```

macOS/Linux shell 可使用：

```bash
export UNIMAIL_GMAIL_CLIENT_ID="你的公开桌面客户端ID"
npm run tauri dev
```

该值可以在运行时提供，也可以在构建安装包时编译进应用。未配置 client ID 时，应用仍能正常构建、安装和使用本地功能，Gmail 设置界面会明确显示当前构建未配置 Gmail 接入。

Unimail V1 请求 `gmail.modify` 与 `gmail.send`，令牌仅写入 Windows Credential Manager 或 macOS Keychain，本地 SQLCipher 数据库只保存不含令牌的凭据引用。真实账号验收步骤见 [`doc/Gmail_Owner_Acceptance.zh-CN.md`](doc/Gmail_Owner_Acceptance.zh-CN.md)。

## Outlook 接入配置

Outlook 使用 Microsoft Entra 公共桌面客户端、系统浏览器和 PKCE，支持个人 Microsoft 账户以及 Microsoft 365 工作/学校账户。桌面应用只使用公开 client ID，绝不接受 client secret。

1. 在 Microsoft Entra 管理中心创建应用注册，将“支持的账户类型”设置为同时允许组织目录账户和个人 Microsoft 账户。
2. 在“身份验证”中添加“移动和桌面应用程序”平台，并启用公共客户端流。回调使用 `http://localhost:{动态端口}/oauth/callback`；Unimail 实际只监听 IPv4 `127.0.0.1`。
3. 添加委托权限 `User.Read`、`Mail.ReadWrite`、`Mail.Send`；登录时还会请求 `offline_access`。
4. 在启动或构建 Unimail 的同一终端设置公开 client ID：

```powershell
$env:UNIMAIL_OUTLOOK_CLIENT_ID="你的公开应用客户端ID"
npm run tauri dev
```

macOS/Linux shell 可使用：

```bash
export UNIMAIL_OUTLOOK_CLIENT_ID="你的公开应用客户端ID"
npm run tauri dev
```

未配置该值时，Outlook 入口会显示安全的未配置状态，Gmail 和本地功能不受影响。Outlook V1 仅同步 Inbox：初次导入最新不超过 500 封邮件，随后使用 Microsoft Graph delta 收敛变化；文件和项目附件支持下载，云端引用附件会明确提示暂不支持。真实账号验收步骤见 [`doc/Outlook_Owner_Acceptance.zh-CN.md`](doc/Outlook_Owner_Acceptance.zh-CN.md)。

## 质量检查

提交前运行：

```powershell
npm run format:check
npm run lint
npm run typecheck
npm test
npm run check:bindings
npm run check:changes
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

`npm run ci:validate` 汇总不依赖交互界面的主要校验，供本地和 GitHub Actions 使用。用户可见变更必须同步更新 [`CHANGELOG.zh-CN.md`](CHANGELOG.zh-CN.md) 的“未发布”章节。

## 构建安装包

```powershell
npm run tauri build
```

Windows 开发机只能原生构建 Windows 安装包。macOS 的 `.app`/`.dmg` 必须在 macOS 上构建，本项目通过 GitHub Actions 的 `macos-latest` 运行器验证；Actions 成功不能替代真实 Mac 上的启动、权限和安装体验测试。

每次普通分支 push 都会在 Windows 与 macOS 原生运行器上执行检查和 Tauri 构建，并上传带 `unsigned` 标识、保留 14 天的工作流产物。普通 push 不会创建 GitHub Release。这些安装包尚未完成 Windows Authenticode 签名、Apple Developer ID 签名或公证，系统可能显示未知发布者或安全提示。

`v*` 标签工作流会先校验标签、项目版本和中文更新日志是否一致，再生成未签名发布候选。创建草稿 Release 的写权限被隔离在最后一个 job，且默认关闭；只有仓库变量 `ENABLE_DRAFT_RELEASE=true` 时才会运行。完整签名、公证、更新器元数据和正式发布将在后续 release integration 工作中实现。

## 凭据与本地数据

不要把以下内容提交到 Git：

- Gmail、Microsoft、QQ、163 等服务商的 OAuth client secret、应用密码或访问令牌
- `.env` 文件、本地配置、本地邮件、缓存和数据库
- Windows/macOS 签名证书、证书密码、Apple API key、notarization 凭据
- Tauri updater 私钥或任何可恢复私钥的材料

需要 CI 使用的服务商配置或签名材料必须存入 GitHub Actions Secrets，并由对应功能的工作流按最小权限读取；本阶段的普通测试构建不要求这些 Secret。

## 发布说明

开发中的用户可见变化写入 `CHANGELOG.zh-CN.md` 的“未发布”章节。发布 `vX.Y.Z` 前：

1. 将相关条目移动到 `## X.Y.Z - YYYY-MM-DD` 章节。
2. 使 `package.json`、根 `Cargo.toml` 的工作区版本和 `src-tauri/tauri.conf.json` 的版本都等于 `X.Y.Z`；`src-tauri/Cargo.toml` 继承工作区版本。
3. 运行 `npm run check:release-tag -- vX.Y.Z`。
