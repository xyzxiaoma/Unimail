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

## 写信、草稿与已发送验收

连接任一支持的邮箱后，可通过“写邮件”或阅读器中的“回复”测试纯文本写信、本地草稿、离线保护和 Sent 对账。跨 Gmail、Outlook、QQ 与 163 的完整手工步骤见 [`doc/Compose_Send_Owner_Acceptance.zh-CN.md`](doc/Compose_Send_Owner_Acceptance.zh-CN.md)；测试时不要记录真实收件人、主题、正文、Message-ID、服务商邮件 ID 或授权信息。

## 质量检查

提交前运行：

```powershell
npm run format:check
npm run lint
npm run typecheck
npm test
npm run check:bindings
npm run check:changes
npm run check:release
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

每次普通分支 push 都会在 Windows 与 macOS 原生运行器上执行检查和 Tauri 构建，并上传带 `unsigned` 标识、保留 14 天的工作流产物。普通 push 没有 Release 写权限，永远不会创建 GitHub Release。这些普通 CI 安装包用于测试，可能显示未知发布者或系统安全提示。

正式发布候选固定为：

- Windows x86_64 NSIS `.exe`
- 同时支持 Apple Silicon 与 Intel 的 macOS Universal `.dmg`

发布工作流支持两种入口：

- `workflow_dispatch`：只读 dry run。校验指定的 `vX.Y.Z`、构建两个平台、验证启动和签名状态、生成完整 payload，但绝不创建 Release。
- 精确的 `vX.Y.Z` tag push：执行同一套校验和组装，之后等待受保护的 GitHub `release` Environment 人工批准，再由唯一拥有 `contents: write` 的 job 创建草稿、上传并核对全部资产，最后一次性公开。

每个公开 Release 都包含安装包、`SHA256SUMS`、`release-provenance.json` 和中文发布说明。只有 Windows Authenticode 与 macOS Developer ID 签名、公证全部验证通过时才会发布稳定版；任一平台使用未签名或 ad-hoc 测试包时，Release 会自动标为 pre-release，并显示中文测试警告。

V1 仅支持手动下载，未安装 Tauri updater 插件，不生成 `.sig`、updater bundle 或 `latest.json`，应用内也不会自动检查、下载或安装更新。

## 凭据与本地数据

不要把以下内容提交到 Git：

- Gmail、Microsoft、QQ、163 等服务商的 OAuth client secret、应用密码或访问令牌
- `.env` 文件、本地配置、本地邮件、缓存和数据库
- Windows/macOS 签名证书、证书密码、Apple API key、notarization 凭据
- Tauri updater 私钥或任何可恢复私钥的材料

需要 CI 使用的服务商配置或签名材料必须存入 GitHub Actions Secrets，并由对应功能的工作流按最小权限读取。普通测试构建和无签名发布候选不要求这些 Secret。

发布工作流识别以下 Secret：

| 平台    | GitHub Actions Secrets                                                                                                     |
| ------- | -------------------------------------------------------------------------------------------------------------------------- |
| Windows | `WINDOWS_CERTIFICATE`、`WINDOWS_CERTIFICATE_PASSWORD`                                                                      |
| macOS   | `APPLE_CERTIFICATE`、`APPLE_CERTIFICATE_PASSWORD`、`APPLE_SIGNING_IDENTITY`、`APPLE_ID`、`APPLE_PASSWORD`、`APPLE_TEAM_ID` |

每个平台的 Secret 必须“全部未配置”或“全部配置”。部分配置会在构建前失败并只报告缺失的变量名，不会静默降级，也不会输出 Secret 值。Windows PFX 和 macOS P12/临时 Keychain 只写入 runner 临时目录并在 `always()` 清理步骤中删除。

## 发布说明

开发中的用户可见变化写入 `CHANGELOG.zh-CN.md` 的“未发布”章节。发布 `vX.Y.Z` 前：

1. 将相关条目移动到 `## X.Y.Z - YYYY-MM-DD` 章节。
2. 使 `package.json`、根 `Cargo.toml` 的工作区版本和 `src-tauri/tauri.conf.json` 的版本都等于 `X.Y.Z`；`src-tauri/Cargo.toml` 继承工作区版本。
3. 运行 `npm run check:release` 和 `npm run check:release-tag -- vX.Y.Z`。
4. 在 GitHub Actions 手动运行“桌面端发布候选与受保护发布”，输入 `vX.Y.Z` 完成 dry run；它不会创建 tag 或 Release。
5. 确认仓库已创建名为 `release` 的 Environment，并配置 required reviewers。没有人工批准保护时，不得创建正式 tag。
6. 所有 dry-run 资产与签名状态确认后，执行：

```powershell
git tag -a vX.Y.Z -m "Unimail vX.Y.Z"
git push origin vX.Y.Z
```

tag 流程到达 `release` Environment 后，由仓库所有者审阅 provenance、签名状态和中文说明，再批准发布 job。实施和测试发布流程时不要提前创建真实 tag。

完整的 Secret 准备、dry run、人工审批、草稿恢复和签名密钥轮换边界见 [发布所有者检查清单](doc/Release_Owner_Checklist.zh-CN.md)。
