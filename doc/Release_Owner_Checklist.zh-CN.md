# Unimail 发布所有者检查清单

本清单用于 Windows x86_64 NSIS 与 macOS Universal DMG 的 GitHub Release。V1 仅支持手动下载，不启用应用内自动更新。

## 一次性仓库设置

1. 在 GitHub 仓库 Settings → Environments 创建名为 `release` 的 Environment。
2. 为该 Environment 配置 required reviewers，确保 tag 构建完成后仍需所有者明确批准。
3. 需要生产签名时，将凭据分别写入 GitHub Actions Secrets：
   - Windows：`WINDOWS_CERTIFICATE`、`WINDOWS_CERTIFICATE_PASSWORD`
   - macOS：`APPLE_CERTIFICATE`、`APPLE_CERTIFICATE_PASSWORD`、`APPLE_SIGNING_IDENTITY`、`APPLE_ID`、`APPLE_PASSWORD`、`APPLE_TEAM_ID`
4. 不准备生产签名时保持对应平台整组 Secret 为空。不要只设置其中一部分。

## 每次版本准备

1. 将 `CHANGELOG.zh-CN.md`“未发布”中的本次内容移动到 `## X.Y.Z - YYYY-MM-DD`。
2. 同步 `package.json`、根 `Cargo.toml` 与 `src-tauri/tauri.conf.json` 的版本。
3. 本地运行：

```powershell
npm ci
npm run check:release
npm run check:release-tag -- vX.Y.Z
npm run ci:validate
git diff --check
```

4. 推送准备提交，但不要创建 tag。
5. 在 Actions 手动运行“桌面端发布候选与受保护发布”，输入 `vX.Y.Z`。确认：
   - Windows 只有一个 x86_64 NSIS 安装包；
   - macOS 只有一个 Universal DMG；
   - 两个平台启动冒烟均通过；
   - `SHA256SUMS`、`release-provenance.json` 与中文说明存在；
   - dry run 没有创建 GitHub Release。

## 创建正式 tag

只有 dry run 通过且签名状态符合预期后执行：

```powershell
git tag -a vX.Y.Z -m "Unimail vX.Y.Z"
git push origin vX.Y.Z
```

不要移动已经推送的发布 tag。版本或说明有误时，修复代码并准备一个新版本。

## 人工批准前复核

发布 job 等待 `release` Environment 审批时，复核：

- tag、版本、commit 与 dry run 一致；
- Windows provenance 为 `authenticode` 或准确的 `unsigned`；
- macOS provenance 为 `developer-id` 且 `notarized=true`，或准确的 `adhoc`；
- 任一平台未完成生产签名/公证时，预期结果是 pre-release；
- 中文说明没有“暂无”、TODO 或占位内容；
- 没有 `latest.json`、`.sig` 或 updater bundle。

批准后，发布器会创建或复用同 tag、同 commit 的私有草稿，上传并核对精确资产集合，再公开 Release。

## 失败恢复

- 构建、启动、签名、公证或组装失败：修复工作流/代码后重新运行；此阶段不会留下公开 Release。
- 发布器创建草稿后失败：保持草稿私有。修复后只允许对同 tag、同 commit 重试；发布器会拒绝不同 commit、意外资产或已经公开的 Release。
- 不要手工公开不完整草稿，不要在 provenance 未验证时改成稳定版。
- 如果必须放弃草稿，在 GitHub Releases 页面确认它仍为 Draft，再由所有者删除；不要删除或移动已公开版本的 tag。

## 证书与密钥轮换

- Windows/Apple 平台证书轮换时，替换对应整组 GitHub Secrets，再先跑一次 dry run 验证新身份。
- 旧证书是否撤销、何时撤销由证书提供方和所有者策略决定；不要把证书或密码提交到仓库。
- V1 没有 updater 密钥。未来启用应用内更新必须另开受审任务：离线生成并备份密钥对，只提交公钥，只把私钥放入 Secret，并验证旧版本接受、新版本篡改拒绝和轮换过渡。
