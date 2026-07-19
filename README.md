# Unimail

Unimail 是一个以简体中文为首发界面语言的跨平台桌面邮件客户端。本仓库当前处于工程基础建设阶段，Windows 和 macOS 安装包仅用于测试，不代表已经完成代码签名、公证或自动更新集成。

## 环境要求

- Node.js 22 与 npm
- Rust stable，并安装 `rustfmt`、`clippy`
- Tauri 2 对应平台的系统依赖
- Windows 本地开发需要 Microsoft C++ Build Tools、Windows SDK 和 WebView2；请按 [Tauri Windows prerequisites](https://v2.tauri.app/start/prerequisites/#windows) 配置

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
