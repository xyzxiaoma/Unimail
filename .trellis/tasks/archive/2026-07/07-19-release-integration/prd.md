# Release Integration

## Goal

Complete a reproducible, secret-safe Windows/macOS release pipeline in which ordinary pushes produce
temporary test artifacts and an exact `v*` tag can create one verified GitHub Release with Chinese
release notes, checksums, accurate signing state, and cryptographically valid updater metadata only
when updater signing is deliberately enabled.

## User Value

- Download one clearly identified Windows installer and one macOS installer from a versioned Release.
- Understand whether each artifact is unsigned/ad-hoc or production-signed/notarized before running it.
- Verify downloads with published SHA-256 checksums.
- Avoid partial Releases, mismatched versions, empty notes, misleading signing claims, or insecure
  updater metadata.

## Confirmed Facts

- The repository uses `main` only. Ordinary pushes already validate, build, launch-smoke, and upload
  unsigned Windows/macOS artifacts for 14 days; they must never create a GitHub Release.
- Release tags use `vX.Y.Z`. The tag, `package.json`, workspace Cargo version,
  `src-tauri/tauri.conf.json`, and an exact non-empty `CHANGELOG.zh-CN.md` version section must match.
- The current tag workflow builds unsigned candidates and can create a disabled-by-default draft
  Release, but it does not yet extract Chinese notes, verify asset completeness, generate checksums
  or provenance, handle signing/notarization Secrets, or safely publish a final Release.
- Platform signing is optional: complete Windows/Apple credentials enable signing; no credentials
  still allow clearly labeled test artifacts. A partial credential set must fail instead of silently
  downgrading.
- macOS without Developer ID credentials uses Tauri's ad-hoc identity `-`; this is neither Developer
  ID trust nor notarization and must be described accurately.
- The first release matrix targets a Windows x86_64 NSIS installer and one macOS Universal DMG that
  supports both Apple Silicon and Intel Macs.
- If either platform lacks verified production signing (Windows Authenticode or macOS Developer ID
  plus notarization), the public GitHub Release is marked as a pre-release and contains an explicit
  Chinese test-build warning. Only a fully verified two-platform build may be marked stable.
- Tauri updater signing is independent of Authenticode and Apple signing. Updater metadata must never
  contain an empty, reused, or fabricated signature. No updater private key may be committed.
- V1 Releases are direct-download-only. The application does not activate an in-app updater, and the
  Release workflow must omit updater bundles, signatures, and `latest.json` until a later task adds a
  deliberately generated and backed-up updater key pair.
- Only a single publisher job may receive `contents: write`. Native matrix jobs remain read-only and
  upload workflow artifacts for the publisher to verify.
- The final publisher runs in a protected GitHub `release` environment and requires explicit owner
  approval before a verified draft can become public. A valid tag alone cannot bypass this gate.
- Failed publication must leave no public partial Release. Assets are assembled and checked before a
  draft is promoted to a public Release.
- GitHub Release creation, tag creation, signing, notarization, and updater-key activation require an
  explicit owner-controlled release action; implementation/testing alone must not publish a Release.

## Requirements

### Tag and release-note contract

- Preserve tag-only publication and reject malformed tags, version drift, missing/empty Chinese
  version notes, placeholders, or an unreleased section used as immutable release notes.
- Extract the exact version section from `CHANGELOG.zh-CN.md` and use it as the Release description.
- Add a generated artifact/security table describing filename, architecture, platform-signing state,
  notarization state, updater availability, and checksums.

### Native build and signing policy

- Build Windows and macOS candidates on native GitHub-hosted runners and run the packaged-executable
  startup smoke before publication.
- Build Windows as x86_64 NSIS and macOS as `universal-apple-darwin`; artifact names and provenance
  must state their architecture explicitly.
- Keep Windows Authenticode credentials and Apple signing/notarization credentials in GitHub Secrets
  only. Temporary certificate/keychain material must live under runner-temporary paths and be removed
  in unconditional cleanup steps.
- Treat complete platform credential sets as production signing requests and verify their output.
  Treat missing sets as explicit unsigned/ad-hoc test builds. Reject partial sets early with safe
  missing-variable diagnostics.
- Publish filenames and provenance that cannot confuse unsigned/ad-hoc artifacts with verified signed
  artifacts.
- Derive GitHub's pre-release/stable state from verified provenance rather than a manually supplied
  label or filename.

### Release transaction

- Build jobs upload installers plus a machine-readable provenance manifest using read-only repository
  permissions.
- A single dependent publisher downloads every artifact, verifies the required platform set,
  filenames, non-zero sizes, startup results, signing state, checksums, version, and tag.
- Generate `SHA256SUMS` and consolidated provenance from the verified files.
- Create or update one draft Release for the exact tag, upload the verified assets, confirm the remote
  asset set, then publish only after all checks pass and the protected `release` environment receives
  explicit owner approval.
- Retries for the same tag must be deterministic and must not create duplicate public Releases or
  silently replace assets from a different commit.

### Updater security boundary

- Never publish `latest.json` or updater signatures for ordinary push artifacts.
- Keep V1 direct-download-only and omit updater bundles, `.sig` files, `latest.json`, updater plugins,
  updater capabilities, and client update prompts.
- Preserve an explicit future boundary: updater activation requires a separate reviewed change that
  generates and backs up a key pair, commits only the public key, stores only the private key in
  GitHub Secrets, and adds tamper/rejection plus previous-version acceptance tests.

### Documentation and repository safety

- Update `README.md` with release preparation, required variables/Secrets, unsigned/ad-hoc warnings,
  failure recovery, key rotation, and exact owner release commands.
- Update `CHANGELOG.zh-CN.md` under `未发布` for user-visible installation/update behavior.
- Add deterministic script tests for release-note extraction, signing-state validation, provenance,
  checksum generation, asset completeness, updater metadata, and partial-secret failures.
- Never commit certificates, private keys, passwords, notarization credentials, `.env` files, or
  generated signing material.

## Acceptance Criteria

- [x] An ordinary `main` push produces temporary Windows/macOS artifacts and no GitHub Release.
- [x] A malformed/mismatched tag or missing Chinese version section fails before native publication.
- [x] Native build jobs have read-only permissions; only the final publisher has `contents: write`.
- [x] The verified asset set contains one Windows x86_64 NSIS installer and one macOS Universal DMG.
- [x] No platform signing Secrets produces accurately labeled unsigned/ad-hoc test assets.
- [x] Any partial Windows or Apple credential set fails early without printing secret values.
- [ ] Complete Windows credentials produce an Authenticode-verified installer with recorded identity.
- [ ] Complete Apple credentials produce a codesign-verified, notarized, stapled artifact; signed but
      unnotarized output cannot be reported as production-ready.
- [x] Any unsigned/ad-hoc platform makes the GitHub Release a pre-release with a Chinese warning;
      stable publication is impossible unless both platform signing states verify as production-ready.
- [x] The publisher requires both platforms, generates SHA-256 checksums and provenance, uses the exact
      Chinese changelog section, and publishes one Release only after verifying the draft asset set
      and receiving protected-environment approval.
- [x] Updater-disabled Releases contain no `latest.json`, updater bundle, or `.sig` masquerading as an
      update channel.
- [x] No updater plugin, capability, endpoint, key, prompt, or automatic download/install behavior is
      activated in the V1 application.
- [x] Release documentation lets the owner configure Secrets, prepare a version, run a dry check,
      create the tag, recover a failed draft, and identify unsigned/ad-hoc artifacts.
- [x] Security, dependency, frontend/Rust, native startup, changed-path, release-note, and release
      workflow tests pass without committed credentials or signing material.

Owner acceptance decision on 2026-07-23: real certificate-backed Authenticode and Apple Developer ID
notarization execution is deferred because no production certificates are available. The owner
explicitly approved completing this task with the fail-closed signing contracts and the verified
unsigned/ad-hoc `v0.1.0` test pre-release; the two unchecked production-credential criteria remain a
future operational acceptance item rather than a blocker for this direct-download test release.

## Out of Scope

- Linux, mobile, Microsoft Store, Mac App Store, Homebrew, Winget, or other package repositories.
- Purchasing or provisioning Windows/Apple certificates on behalf of the owner.
- Creating a production tag or publishing a real GitHub Release before the owner explicitly approves
  the prepared release commit and signing/update state.
- In-app update checks, updater downloads/install/restart behavior, updater key generation, and
  `latest.json`; these require a separate owner-approved task.
- Automatic rollback of a database migration after an application downgrade.

## Open Questions

- None blocking planning.
