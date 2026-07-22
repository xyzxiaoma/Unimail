# Release Integration Implementation Plan

## 1. Baseline and Release Inventory

- Run `trellis-before-dev` and load quality, logging, cross-layer, changelog, parent release research,
  and native-startup contracts.
- Record the current ordinary-push and tag workflow behavior, GitHub permissions, Tauri bundle
  targets, action versions, release scripts, artifact paths, and secret/path gates.
- Confirm ordinary push run `29911825929` remains the unsigned native-build/startup baseline.

## 2. Deterministic Release Contract Scripts

- Refactor the existing tag validator only where needed; preserve exact tag/version/changelog checks.
- Add exact Chinese version-note extraction.
- Add release provenance schema validation, signing-state classification, stable filename derivation,
  pre-release derivation, checksum generation, release-body rendering, and exact asset-set checks.
- Reject unexpected/updater assets (`latest.json`, updater bundles, `.sig`) in V1 payloads.
- Add fictional self-tests covering good/base/bad cases, malformed manifests, cross-commit/tag drift,
  partial secret sets, tampered checksums, missing platforms, duplicate assets, and updater leakage.
- Register a single local `npm run check:release` command used by CI and tag validation.

Validation:

```powershell
npm run check:release
npm run check:release-tag -- v0.1.0
```

The successful tag command requires a temporary fictional fixture or the prepared real version
section; do not mutate the production changelog merely to make a local happy-path test pass.

## 3. Release Staging and Native Startup Paths

- Extend `scripts/check-native-startup.ps1` with an explicit optional target root or equivalent exact
  resolver that supports `target/release` and `target/universal-apple-darwin/release`.
- Add a staging helper that copies only the verified installer into a clean runner-temporary directory
  using the deterministic release filename and writes the per-platform provenance manifest.
- Ensure staging never includes raw certificates, `.sig`, logs, runner paths, or unrelated bundle
  outputs.
- Test Windows resolution locally and use fictional directory fixtures for macOS universal/app-cleanup
  resolution where practical.

## 4. Windows Optional Authenticode Path

- Add an early PowerShell secret-state step using only empty/non-empty booleans.
- Fail on a partial `WINDOWS_CERTIFICATE`/`WINDOWS_CERTIFICATE_PASSWORD` pair.
- For a complete pair, decode/import the PFX under `RUNNER_TEMP`, derive the imported code-signing
  thumbprint, create a temporary Tauri config merge, and make signing mandatory.
- After NSIS build, verify Authenticode status and certificate identity; record `authenticode` only on
  success. With no pair, record `unsigned` and never claim signature validity.
- Clean the PFX and imported certificate with `if: always()`.

## 5. macOS Universal Optional Signing/Notarization Path

- Install both Apple Rust targets and build `--target universal-apple-darwin`.
- Classify the six-value Apple production set as absent, complete, or partial; fail partial state.
- With no set, use ad-hoc signing identity and record `adhoc`, `notarized=false`.
- With a complete set, expose credentials only to the build step, require Developer ID signing and
  notarization, then verify codesign, Gatekeeper, and stapling before recording production state.
- Run the universal-target native startup smoke and stage one Universal DMG.
- Clean temporary certificate/keychain material unconditionally.

## 6. Read-only Release Assembly

- Download the two native candidate artifacts into a clean directory.
- Validate exact platform/version/tag/commit/architecture/signing/startup fields and exact installer
  count before reading filenames into the payload.
- Generate deterministic `SHA256SUMS`, consolidated `release-provenance.json`, and
  `release-notes.zh-CN.md` with the Chinese changelog section and signing/security table.
- Derive `prerelease=true` unless Windows Authenticode and macOS Developer ID+notarization all verify.
- Upload one `release-payload-<tag>-<sha>` workflow artifact; keep this job `contents: read`.

## 7. Protected Single Publisher

- Replace the disabled `ENABLE_DRAFT_RELEASE` skeleton with a tag-only publisher depending on the
  assembled payload.
- Run it in the protected `release` environment; document that repository setup must enable required
  reviewers before production use.
- Grant `contents: write` only to this job after approval.
- Revalidate tag/commit/payload, create or safely reuse a same-tag/same-commit draft, upload exact
  assets, query the remote asset set, then publish with derived pre-release/stable state.
- Refuse existing public Releases, moved tags, unexpected assets, cross-commit drafts, or incomplete
  remote uploads. Do not create a Release for `workflow_dispatch` dry runs.

## 8. Workflow Supply-chain and Dry Run

- Add `workflow_dispatch` release-candidate dry runs that accept a version tag for validation/build
  but cannot publish.
- Pin release-workflow third-party actions to reviewed full commit SHAs with readable version comments.
- Preserve fixed Node 22 and Rust 1.95.0, lockfile installs, dependency audits, security checks, native
  startup checks, and artifact `if-no-files-found: error`.
- Add workflow/static assertions for tag-only publication, environment name, permissions, action pins,
  target architectures, cleanup steps, and absence of updater configuration.

## 9. Documentation and Changelog

- Update `README.md` with Windows/macOS targets, direct-download-only status, pre-release rules,
  required environment protection, all supported signing Secrets, partial-secret failure, dry runs,
  version preparation, tag creation, failed-draft recovery, and owner approval.
- Update `CHANGELOG.zh-CN.md` under `未发布` with the user impact of verified Releases, checksums,
  architecture coverage, and accurate pre-release/signing labels.
- Add a release owner checklist if README would become too operationally dense.
- Update AGENTS/spec contracts only when the implementation establishes new executable rules beyond
  the existing tag-only/no-secret constraints.

## 10. Integrated Validation

Run locally:

```powershell
npm run format:check
npm run lint
npm run typecheck
npm test
npm run check:bindings
npm run check:changes
npm run check:security:self-test
npm run check:security
npm run check:release
npm audit --omit=dev
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo audit
cargo deny check --warn unmaintained
git diff --check
```

Also verify in Actions:

- ordinary push still creates no Release;
- manual dry run creates a complete candidate payload and no Release;
- macOS Universal DMG builds and launches;
- unsigned/ad-hoc candidate derives `prerelease=true`;
- partial-secret fixtures fail before build;
- a real tag/publisher run is not executed until the owner explicitly approves release preparation.

## 11. Review and Rollback Points

- Review manifest fields and stable/pre-release derivation before editing workflow publication.
- Review secret-state and cleanup logic before exposing any repository Secrets to native jobs.
- Review universal target paths before replacing current macOS candidate globs.
- Keep the publisher disabled from real tag use until dry-run assembly and protected environment setup
  are verified.
- If signed-output verification cannot be proven, publish only accurately labeled pre-release test
  artifacts; never downgrade a claimed signed build.
- If the publisher fails after draft creation, keep the draft private and retry only for the same
  tag/commit after fixing the workflow.

## 12. Completion Gate

- Run `trellis-check` and update executable release/quality specs.
- Commit and push only after the full local gate passes.
- Require Windows/macOS release-candidate dry-run artifacts and checks to pass.
- Do not create the first real `v0.1.0` tag or public Release until the owner separately approves the
  prepared version/changelog, signing state, protected environment, and final publication action.
