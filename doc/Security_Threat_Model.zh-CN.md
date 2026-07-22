# Unimail V1 安全威胁模型

本文描述 Windows 与 macOS V1 的受信边界、主要威胁和验证入口。Linux、签名、公证、更新器
和正式 Release 发布由后续任务单独处理。

## 数据与信任边界

- 邮件 MIME、HTML、附件名、服务商响应、外部链接和 OAuth 回调请求均不可信。
- React WebView 只调用显式注册的 Tauri 用例命令，不拥有通用网络、文件系统、Shell、进程、
  对话框或更新器权限。
- 邮件数据只进入 SQLCipher；数据库密钥、OAuth 令牌和 QQ/163 授权码只进入系统凭据存储。
- 服务商网络访问、外部浏览器打开和附件保存位置选择均由 Rust 边界控制。

## 威胁与控制

| 威胁 | 主要控制 | 自动验证 |
| --- | --- | --- |
| WebView 注入后扩大系统权限 | 精确 Tauri capability、显式命令、禁止新窗口和任意导航 | `npm run check:security`、Tauri 单元测试 |
| HTML 邮件执行脚本、表单或导航 | DOMPurify 固定白名单、`sandbox=""`、内嵌 `default-src 'none'` CSP | `SafeHtmlMessage` 恶意语料测试 |
| 远程图片跟踪或 SSRF | 默认阻止、显式批准、HTTPS、公网 IP、每跳 DNS/重定向复核、大小和类型限制 | `remote_image` Rust 测试 |
| 外部链接绕过确认或携带凭据 | React 确认界面、Rust 仅接受无凭据 HTTP(S) URL | 外链 IPC 与组件测试 |
| OAuth 回调伪造、重放或过大请求 | PKCE、随机一次性 state、回环地址、固定路径、请求上限、超时和取消 | Gmail/Outlook OAuth 测试 |
| Provider 返回恶意游标、错误或正文 | 固定端点、边界解析、Opaque/红acted 类型、固定错误码、禁止原文输出 | Provider conformance 与适配器测试 |
| 本地数据库或凭据泄露 | SQLCipher 256 位密钥、系统凭据存储、macOS owner-only 文件权限 | SQLCipher、凭据和权限测试 |
| 附件路径穿越、覆盖或残留 | 后端附件 ID、清理文件名、`create_new`、no-clobber、重启账本、无成功私有副本 | 附件存储/应用/Tauri 测试 |
| 账户删除遗漏本地数据 | 凭据→数据库→附件持久化清理状态机与重启恢复 | Repository 清理测试 |
| 日志或诊断泄露隐私 | 无运行时日志/遥测、固定 DTO、仅计数诊断、源码输出检查 | `check:security`、IPC decoder 测试 |
| 提交凭据或本地邮件 | `.gitignore`、变更路径检查、高置信内容秘密扫描 | `npm run check:changes`、`check:security` |
| 已知依赖漏洞或不允许许可证 | npm audit、RustSec、Cargo license/source policy、锁文件 | CI security job |

## 安全诊断分享规则

可以分享应用版本、平台、联网状态、SQLCipher/FTS5/系统凭据存储状态，以及每个服务商的配置
与账户状态数量。不得分享邮箱地址、显示名称、任何 ID、邮件内容、收件人、搜索词、游标、令牌、
凭据引用、本地路径、主机名或环境变量。

## 外部验收边界

- Gmail、Outlook、QQ 和 163 的真实账户行为由仓库所有者使用专用测试账户验证。
- macOS Keychain 和 Unix 文件权限由 macOS 原生 CI/设备验证。
- Windows/macOS 安装包签名、公证、更新器密钥和正式发布由 `release-integration` 处理。
