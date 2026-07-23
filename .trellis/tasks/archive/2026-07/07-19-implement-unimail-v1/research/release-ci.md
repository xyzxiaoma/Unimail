# Research: GitHub Actions and Tauri 2 release design

- Query: Design a GitHub Actions/Tauri 2 pipeline where every push builds Windows and macOS installers as workflow artifacts, `v*` tags create official GitHub Releases, Windows/Apple signing is optional with a clearly identified unsigned fallback, Chinese release descriptions are repository-maintained and AI-enforced, and automatic-update metadata remains cryptographically secure.
- Scope: mixed
- Date: 2026-07-19

## Findings

### Repository facts and requirements

- `.trellis/tasks/07-19-implement-unimail-v1/prd.md:32-35` defines the two delivery levels: every push builds native Windows/macOS artifacts; `v*` tags publish official Releases; platform signing is conditional; Chinese descriptions are maintained in the repository.
- `.trellis/tasks/07-19-implement-unimail-v1/prd.md:86` requires release/update artifacts to use Tauri and platform verification mechanisms.
- `.trellis/tasks/07-19-implement-unimail-v1/prd.md:95-100` requires native macOS infrastructure, a structured Chinese source, AI rules, tag-only Releases, and secret-only optional platform credentials.
- `.trellis/tasks/07-19-implement-unimail-v1/prd.md:119-123` makes the fallback, artifacts, Release description, and missing-note check acceptance criteria.
- `AGENTS.md:19` says content outside the Trellis-managed block is preserved, so the project-specific release-note rule should be added outside that block rather than inside it.
- No `.github/`, `package.json`, or `src-tauri/` exists yet. All workflow/config paths below are proposed implementation targets, not existing patterns.

### Recommended architecture

Use one workflow, `.github/workflows/desktop-release.yml`, with `workflow_dispatch` plus a `push` trigger covering both branches and `v*` tags. Do not create a Release from ordinary branch pushes.

```text
push branch or v* tag
        |
        +--> validate (lint/typecheck/tests/version/release-note gate)
        |
        +--> native build matrix
        |      +-- windows-latest, x86_64, NSIS (+ optional MSI)
        |      +-- macos-latest, universal-apple-darwin, DMG
        |      +-- always upload workflow artifacts
        |
        +--> publish-release (only refs/tags/v*)
               +-- download both jobs' artifacts
               +-- verify completeness/checksums/signing status
               +-- generate complete latest.json once
               +-- create one GitHub Release with Chinese notes
               +-- upload installers, updater bundles/signatures,
                   latest.json, SHA256SUMS
```

Why a single publisher job: a platform matrix in which each `tauri-action` invocation creates/updates the same Release can race on Release body and `latest.json`. Native jobs should build and upload temporary workflow artifacts; one dependent job should create/finalize the permanent Release only after every required platform succeeds.

Recommended job permissions:

- Workflow default: `permissions: contents: read`.
- Native build jobs: no write permission.
- `publish-release`: `permissions: contents: write`; this is the only job receiving `GITHUB_TOKEN` write capability.
- Add `concurrency: release-${{ github.ref }}` with `cancel-in-progress: false` for tags, preventing duplicate tag runs from replacing partially uploaded assets.
- Protect the release environment (for example `production-release`) if repository policy should require manual approval before production Secrets are exposed.

### Event and job rules

- Trigger branch pushes with `push.branches: ['**']` and release tags with `push.tags: ['v*']`; add `workflow_dispatch` for dry runs. Tag pushes are intentionally included in the same workflow.
- `validate` runs for every event.
- Both native build jobs run for every push and upload installer outputs even when no signing Secrets exist.
- `publish-release` uses `if: startsWith(github.ref, 'refs/tags/v')` and depends on all validation/build jobs. Ordinary pushes never call `gh release create` or grant `contents: write`.
- Before a tag build, verify `github.ref_name` is exactly `v${applicationVersion}` and that the same version appears in the chosen canonical application version source and generated manifests. Reject malformed/non-SemVer tags and version drift before expensive builds.
- Prefer `npm ci`, locked Rust dependencies (`Cargo.lock` committed), Node LTS, stable Rust, `swatinem/rust-cache@v2`, and native GitHub-hosted runners.
- Current official Tauri documentation and action README use `tauri-apps/tauri-action@v1`; the latest release observed on 2026-07-19 is `action-v1.0.0` (published 2026-06-29). For supply-chain control, pin third-party actions to reviewed full commit SHAs and let Dependabot propose updates; if the project chooses readable major tags, use current majors and treat updates as reviewed dependency changes.

`tauri-action@v1` can build without Release inputs and has `uploadWorkflowArtifacts: true`, `workflowArtifactNamePattern`, `artifactPaths`, `uploadUpdaterJson`, and updater-signature support. It is acceptable to use its workflow-artifact feature, but explicit `actions/upload-artifact` around known bundle paths is easier to audit and to label as signed/unsigned. Do not rely on a mutable glob that silently succeeds with no files: set `if-no-files-found: error` (or equivalent explicit path validation).

### Platform build outputs

Windows recommendation:

- Primary V1 installer: NSIS `.exe`, x86_64. Add WiX `.msi` only if product distribution needs it; if both are built, define which one the updater uses (`updaterJsonPreferNsis: true` is available in `tauri-action@v1`).
- Upload the installer on every push with a name such as `Unimail-windows-x86_64-signed` or `Unimail-windows-x86_64-unsigned`.
- On tag builds also retain the Tauri updater `.zip` and `.sig` generated by `bundle.createUpdaterArtifacts: true`.

macOS recommendation:

- Build on `macos-latest` using `--target universal-apple-darwin`, after installing both `aarch64-apple-darwin` and `x86_64-apple-darwin`. This produces one DMG supporting Intel and Apple Silicon. If build time becomes excessive, split the architectures, but then the metadata must contain both targets and Release assets must make architecture obvious.
- With no Apple certificate, configure Tauri's documented ad-hoc identity `-`; this avoids Apple Silicon downloads being treated as structurally unsigned/damaged, but it is not Developer ID trust or notarization. Label it `adhoc-not-notarized`, and describe the Gatekeeper limitation in Chinese.
- With complete Apple Secrets, create/import the Developer ID certificate, sign, notarize, and staple before artifact upload. A signed but unnotarized production artifact must be treated as failure, not as a successful signed result.

Every build should produce a small machine-readable manifest (for example `build-provenance.json`) containing commit SHA, version, OS/arch, installer filenames, `platformSigning` (`signed`, `adhoc`, `unsigned`), `notarized`, and whether updater artifacts are present. The publisher consumes these manifests rather than inferring security state from filenames.

### Optional Windows signing with safe fallback

Tauri's official Windows guide documents a base64 PFX (`WINDOWS_CERTIFICATE`) and password (`WINDOWS_CERTIFICATE_PASSWORD`), importing it into `Cert:\CurrentUser\My`, then configuring `bundle.windows.certificateThumbprint`, SHA-256 digest, and a certificate-provider timestamp URL. It also supports `bundle.windows.signCommand` for Azure Artifact Signing/Key Vault or another signing tool.

Recommended Secrets for the simple PFX route:

- `WINDOWS_CERTIFICATE`: base64 PFX.
- `WINDOWS_CERTIFICATE_PASSWORD`: PFX export password.
- Optional repository variable/config: timestamp URL. This is not secret, but it should be explicit and HTTPS when the provider supports it.

Decision logic:

1. Secrets cannot be referenced directly in a GitHub Actions `if:`. Map them to job/step environment variables, then set a non-secret boolean output in an early PowerShell step.
2. If both PFX values are empty, continue unsigned and set the artifact manifest/name to `unsigned`.
3. If only one value is present, fail immediately as a configuration error. Never silently downgrade a partially configured production signer.
4. If both are present, decode into `$RUNNER_TEMP`, import to the ephemeral current-user store, discover/validate the code-signing certificate and thumbprint, inject signing configuration at build time (a separate CI Tauri config or `--config` merge), and remove temporary material in an `always()` cleanup step.
5. Verify the produced `.exe`/`.msi` with `Get-AuthenticodeSignature`; a configured signing path must fail unless status is valid and the expected publisher certificate is used.

Do not commit a certificate thumbprint that assumes one specific runner store unless that is an intentional certificate identity contract. Runtime discovery plus expected subject/thumbprint validation is safer for certificate rotation. Azure Artifact Signing with OIDC is a preferable later production option because it avoids exporting a long-lived PFX private key, but the PFX route satisfies the optional-Secret requirement and follows Tauri's documented GitHub Actions pattern.

### Optional Apple signing/notarization with safe fallback

Tauri's official macOS guide documents:

- Signing: `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`, and `APPLE_SIGNING_IDENTITY`.
- Notarization via Apple ID: `APPLE_ID`, app-specific `APPLE_PASSWORD`, and `APPLE_TEAM_ID`.
- Notarization via App Store Connect API: `APPLE_API_ISSUER`, `APPLE_API_KEY`, and `APPLE_API_KEY_PATH` after writing the private key to a temporary file.
- Ad-hoc fallback: `bundle.macOS.signingIdentity: '-'`.

Recommended initial route: Developer ID certificate plus Apple ID app-specific password because it maps directly to documented Tauri environment variables. Prefer App Store Connect API credentials when the owner is ready to manage the key file securely.

Decision logic mirrors Windows:

1. No Apple signing/notarization Secrets: build with ad-hoc identity, do not notarize, label artifact `adhoc-not-notarized`, and publish a Chinese warning for test use.
2. Any partial credential set: fail with a list of missing names. Do not publish an artifact that claims production signing.
3. Complete set: expose credentials only to the macOS build step, sign/notarize/staple, then verify with `codesign --verify --deep --strict --verbose=2` and `spctl --assess --type execute --verbose=4`; optionally validate notarization/stapling with `xcrun stapler validate`.
4. Clean temporary keychain/API key files with `always()`. Never print identities together with secret values or enable shell tracing.

Notarization requires a paid Apple Developer identity; the PRD correctly treats actual production execution as dependent on owner-supplied credentials (`prd.md:25,135`).

### Chinese release-note source and AI/repository enforcement

Recommended repository contract:

- `CHANGELOG.zh-CN.md` is the human-maintained source, with `## [未发布]` plus version sections `## [1.2.3] - YYYY-MM-DD` and stable categories such as `新增`, `改进`, `修复`, `安全`, `兼容性`, `已知问题`.
- `scripts/extract-release-notes.mjs <version>` extracts exactly the matching version section into a temporary Markdown file. The tag workflow must fail if the section is missing, empty, still contains placeholders, or the version does not match the tag.
- The GitHub Release body should be the extracted Chinese section, followed by an automatically generated artifact/security table: Windows signing state, macOS signing/notarization state, supported architectures, SHA-256 checksum link, and an explicit test-build warning when platform signing is absent.
- Add a short project rule outside the managed block in `AGENTS.md`: every user-visible feature, fix, security change, or compatibility change must update `CHANGELOG.zh-CN.md` under `未发布`; internal-only changes must state why no note is needed. This placement follows `AGENTS.md:19`.
- Add `.github/pull_request_template.md` checkboxes for release-note category and Chinese wording, but do not rely on checkboxes alone.
- Add `scripts/check-release-note.mjs` to CI. Compare the push range (`github.event.before..github.sha`, with an empty-tree fallback for the first push). If likely user-visible paths changed but neither `CHANGELOG.zh-CN.md` nor an explicit reviewed no-note record changed, fail. Likely paths include frontend UI/copy, Tauri commands/capabilities, provider behavior, security policy, installer/updater config, and compatibility code. Tests/docs-only and mechanical dependency changes can use a documented no-note record with rationale.
- Make the release-note check a required branch-protection status. This turns the AI instruction into an executable rule and satisfies `prd.md:123` better than prompt text alone.

The final release-preparation change moves reviewed entries from `未发布` to the exact version section before the `vX.Y.Z` tag is created. The tag workflow consumes the tagged commit, so the description is immutable and reproducible.

### Tauri updater metadata and security boundary

Tauri 2 updater security is separate from Windows Authenticode and Apple Developer ID signing. Platform signing may legitimately fall back for test installers; updater signing must not silently fall back.

Recommended configuration/flow:

- Add `tauri-plugin-updater` and `tauri-plugin-process`; grant only required updater/process capabilities.
- In Tauri config, set `bundle.createUpdaterArtifacts: true`, embed the updater public key in `plugins.updater.pubkey` (the public key is safe to commit), and use the HTTPS endpoint `https://github.com/xyzxiaoma/Unimail/releases/latest/download/latest.json`.
- Keep `dangerousInsecureTransportProtocol` absent/false. Tauri enforces TLS in production for updater endpoints.
- Store only `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` as Secrets. Back up the private key outside GitHub; loss prevents publishing updates trusted by installed clients. Rotation requires an application release that trusts the new public key before updates are signed only with the new key.
- For every official updater-enabled tag, native builds create updater bundles and `.sig` files. The single publisher job generates one complete `latest.json` after all required targets are available. Required per-platform fields are `url` and the literal contents of the corresponding `.sig`; top-level `version` is SemVer, `notes` is the Chinese version section, and `pub_date` is RFC 3339.
- Validate every URL points to the exact tagged Release, every signature is nonempty, every configured platform entry is complete, and version equals the tag. Tauri validates the whole static JSON before version comparison, so one malformed platform entry can break updates for all clients.
- Generate `SHA256SUMS` for human/manual download verification, but do not confuse checksums with updater authenticity; the embedded public key plus `.sig` is the security control.
- Do not publish `latest.json` for ordinary push artifacts. CI builds are downloadable test artifacts, not an update channel.

Updater signing policy recommendation:

- Branch pushes: updater key may be absent; produce installers and skip updater artifacts/metadata, clearly marking them CI test builds.
- `v*` official Releases: require the updater signing key if in-app updates are enabled in the shipped application. Missing updater key should fail before Release creation. This is intentionally stricter than optional Windows/Apple signing because publishing unsigned updater metadata would violate the embedded-key trust model.
- If the owner explicitly wants an emergency direct-download Release without updater support, require a deliberate workflow input/environment approval and omit `latest.json`; never generate blank signatures or reuse an old signature.

### Release transaction and failure handling

Recommended tag transaction:

1. Validate tag/version/release-note section and signing-secret completeness.
2. Build/verify both native installers; always upload workflow artifacts for diagnosis.
3. If a platform signer is absent, mark the corresponding build as test/unsigned; if partially configured, fail.
4. Generate checksums and complete updater metadata only after all artifacts exist.
5. Create a draft Release, upload all assets, re-download or query assets to confirm required filenames/sizes, then publish it. A failed job leaves no public partial Release; cleanup/retry may reuse the draft for the same tag.
6. Use `gh release create/edit/upload` or the GitHub API in the single publisher. If `tauri-action` is used for the build, omit its `tagName`, `releaseName`, and `releaseId` in native jobs so it cannot create Releases.

Recommended minimum release asset set:

- Windows installer (`.exe`, optionally `.msi`).
- macOS universal `.dmg`.
- Tauri updater bundle(s) plus `.sig` for each supported updater target.
- `latest.json` only for official updater-enabled Releases.
- `SHA256SUMS` and `build-provenance.json`.

### Verification checklist

- Push a normal commit with no Secrets: both installer jobs pass, artifacts are visibly unsigned/ad-hoc, and no GitHub Release exists.
- Configure one member of a credential pair only: workflow fails early and names the incomplete platform configuration.
- Configure Windows credentials: Authenticode verification passes and artifact manifest says signed.
- Configure Apple credentials: codesign, Gatekeeper assessment, notarization, and stapling checks pass.
- Push `vX.Y.Z` with mismatched app version or missing Chinese version section: no public Release is created.
- Push a valid tag: one Release appears with both platforms, the exact Chinese section, checksums, and accurate security-state table.
- Download `latest.json`, validate JSON/schema/URLs, and test Tauri update acceptance from the preceding version. Tamper with an updater bundle or signature in a test fixture and confirm the client rejects it.
- Change a user-visible file without a changelog/no-note record and confirm the required CI check fails.

### Files found

- `.trellis/tasks/07-19-implement-unimail-v1/prd.md` — authoritative V1 delivery, signing, release-note, and updater acceptance requirements.
- `AGENTS.md` — Trellis-managed instruction block and preservation rule for project-specific instructions outside it.
- `.trellis/workflow.md` — Trellis planning/research persistence and task workflow.
- `.trellis/spec/guides/index.md` — flags config changes and cross-layer work for explicit review (`index.md:37,47`).
- `.trellis/spec/guides/cross-layer-thinking-guide.md` — requires explicit contracts at boundaries and verification across consumers (`cross-layer-thinking-guide.md:46-47,105-117`), applicable to tag/version/notes/artifacts/updater metadata.
- `doc/Unimail_Product_Specification_v1.0.md` — product source; no additional concrete CI implementation exists there.

### External references

- Tauri 2 GitHub pipeline guide: https://v2.tauri.app/distribute/pipelines/github/ — native matrix, tag triggers, Rust/Node setup, and `tauri-action` release pattern.
- `tauri-apps/tauri-action` README: https://github.com/tauri-apps/tauri-action — `v1` inputs including workflow artifacts, release uploads, updater JSON/signatures, output paths, and the rule that omitting Release inputs performs build-only operation. Latest observed release: `action-v1.0.0`.
- Tauri Windows signing: https://v2.tauri.app/distribute/sign/windows/ — PFX import/thumbprint, timestamping, custom sign command, Azure Key Vault/Artifact Signing.
- Tauri macOS signing/notarization: https://v2.tauri.app/distribute/sign/macos/ — certificate/identity variables, Apple ID or App Store Connect notarization variables, and ad-hoc identity `-`.
- Tauri updater plugin: https://v2.tauri.app/plugin/updater/ — updater key generation, `createUpdaterArtifacts`, HTTPS endpoints, static JSON schema, signatures, target naming, and permissions.
- GitHub Actions encrypted Secrets: https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/use-secrets — Secrets are unavailable in some contexts and cannot be directly referenced in `if:`; map them to environment variables for conditional steps.
- GitHub artifact upload: https://github.com/actions/upload-artifact — workflow-artifact retention/behavior. Latest observed release on 2026-07-19: `v7.0.1`; pin the chosen action version/SHA consistently with the runner platform policy.

### Related specs

- `.trellis/spec/guides/index.md`
- `.trellis/spec/guides/cross-layer-thinking-guide.md`
- `.trellis/spec/backend/index.md` and `.trellis/spec/backend/quality-guidelines.md` exist but are scaffolds and contain no established release conventions.

## Caveats / Not Found

- The repository has no application source, version source, Tauri configuration, package manager lockfile, GitHub workflow, release-note source, or existing updater public key. Exact commands and glob paths must be finalized after the desktop scaffold establishes its layout.
- GitHub-hosted runner labels and action major versions evolve. The versions above reflect official sources observed on 2026-07-19; pin reviewed SHAs during implementation and verify supported Node runtime/runners then.
- Windows certificate providers differ. The documented PFX approach is straightforward but newer OV/EV certificates may require hardware/cloud signing; Tauri explicitly directs newer certificates to the issuer/custom-sign-command path.
- Apple production notarization cannot be proven without owner-supplied paid Developer ID credentials. The no-credential fallback is ad-hoc and must never be described as Apple-notarized.
- GitHub Release assets are public distribution objects, whereas workflow artifacts are temporary CI outputs. Retention days and repository visibility must be chosen explicitly; neither should contain Secrets or unredacted logs.
- Automatic updater key rotation is not solved by CI alone. The application needs an explicit trust-transition release, and the private key needs an owner-controlled backup/recovery procedure.
