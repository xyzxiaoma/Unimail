# Release Integration Technical Design

## 1. Design Goals

- Preserve the existing rule that ordinary pushes produce temporary artifacts and never Releases.
- Turn an exact `vX.Y.Z` tag into one reproducible, manually approved GitHub Release transaction.
- Support Windows x86_64 NSIS and macOS Universal DMG from native runners.
- Use optional platform signing without allowing partial configuration or misleading trust claims.
- Keep V1 direct-download-only: no updater plugin, key, metadata, prompt, or automatic install path.
- Make release notes, provenance, checksums, asset completeness, and stable/pre-release state
  deterministic and testable without real signing credentials.

## 2. Current State and Gaps

The repository already has:

- exact tag/version/changelog validation;
- native Windows/macOS release-candidate builds;
- full quality/security/dependency gates;
- packaged-executable startup smoke checks;
- temporary candidate artifact upload;
- a disabled draft-Release skeleton with isolated `contents: write`.

The current workflow still lacks version-note extraction, universal macOS output, signing-secret
state validation, signed-output verification, stable asset names, checksums, provenance, artifact-set
verification, deterministic retry behavior, pre-release derivation, and protected-environment approval
for public publication.

## 3. Release State Machine

```text
ordinary main push
  -> validate + native build + startup smoke
  -> 14-day unsigned/ad-hoc workflow artifacts
  -> no Release permissions and no GitHub Release

workflow_dispatch dry run
  -> validate supplied vX.Y.Z against checked-out commit
  -> release build matrix + startup/signing verification
  -> assembled release payload workflow artifact
  -> no GitHub Release

vX.Y.Z tag push
  -> validate exact tag/version/Chinese version notes
  -> native build matrix (read-only)
  -> assemble and verify complete release payload (read-only)
  -> protected release environment approval
  -> single publisher creates draft, uploads/verifies assets, publishes once
```

Any validation, build, signing, startup, completeness, checksum, or remote-asset failure stops before
public publication. The protected publisher never consumes artifacts from a different tag commit.

## 4. Canonical Release Contracts

### Version notes

`CHANGELOG.zh-CN.md` remains the source of truth. A release-notes extractor accepts `X.Y.Z`, selects
the exact `## X.Y.Z - YYYY-MM-DD` section, rejects placeholders/empty sections, and writes immutable
Chinese Markdown for the publisher. `未发布` is never used as a tagged Release body.

### Per-platform provenance

Each native job stages one installer and one JSON manifest:

```json
{
  "schemaVersion": 1,
  "platform": "windows",
  "version": "0.1.0",
  "tag": "v0.1.0",
  "commit": "<40-hex-sha>",
  "architecture": "x86_64",
  "installerKind": "nsis",
  "installerFile": "Unimail_0.1.0_windows_x86_64_unsigned.exe",
  "platformSigning": "unsigned",
  "notarized": false,
  "nativeStartupPassed": true
}
```

Allowed states are exact:

- Windows: `platformSigning = unsigned | authenticode`, `notarized = false`.
- macOS: `platformSigning = adhoc | developer-id`; `notarized = true` only for verified
  `developer-id` output.
- Windows architecture is `x86_64`; macOS architecture is `universal`.

Manifests contain no runner paths, environment values, secret presence details, certificate blobs,
credentials, usernames, or hostnames.

### Consolidated release payload

The assembly job validates both manifests against the event tag and commit, verifies the exact
installer set and non-zero files, renames/stages assets deterministically, and writes:

- Windows x86_64 NSIS installer;
- macOS Universal DMG;
- `SHA256SUMS` sorted by filename;
- `release-provenance.json` containing the two validated manifests plus asset hashes;
- `release-notes.zh-CN.md` containing exact version notes and a generated security table.

The payload also derives `prerelease=true` unless Windows is `authenticode` and macOS is
`developer-id` with `notarized=true`.

## 5. Native Matrix and Signing

### Windows

- Build on `windows-latest` using the native x86_64 toolchain and NSIS target.
- Map `WINDOWS_CERTIFICATE` and `WINDOWS_CERTIFICATE_PASSWORD` Secrets to step environment values.
- Both empty means unsigned; exactly one set means configuration failure; both set means signing is
  mandatory.
- Decode the PFX only under `RUNNER_TEMP`, import it into an ephemeral current-user certificate store,
  derive the imported code-signing thumbprint, and merge a temporary Tauri signing config.
- After build, require `Get-AuthenticodeSignature` to report `Valid` and the expected imported
  certificate thumbprint before provenance may say `authenticode`.
- Remove the temporary PFX and imported certificate in an `always()` cleanup step.

### macOS

- Build on `macos-latest`; install `aarch64-apple-darwin` and `x86_64-apple-darwin`, then build
  `--target universal-apple-darwin`.
- The complete production set is `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID`.
- All empty means ad-hoc identity `-`; any partial set fails; a complete set requires Developer ID
  signing and notarization.
- Production output must pass `codesign --verify --deep --strict`, Gatekeeper assessment, and stapler
  validation. Only then may provenance report `developer-id` and `notarized=true`.
- Temporary certificate/keychain material stays on the runner and is cleaned unconditionally.

Signing Secrets are available only to their native read-only build jobs. They never reach the
assembly or publisher jobs and are never used in shell tracing or diagnostic dumps.

## 6. Native Startup and Target Paths

The existing startup checker is extended to resolve an optional release target root. Windows keeps
`target/release`; macOS release builds use `target/universal-apple-darwin/release`. On macOS the script
prefers an available `.app/Contents/MacOS` binary and falls back to the retained release binary after
DMG packaging removes the intermediate app.

The build job records `nativeStartupPassed=true` only after the process remains alive for the bounded
smoke interval. The manifest generator refuses caller-supplied `true` unless the workflow step that
creates the manifest depends on the successful smoke step.

## 7. Publisher and Manual Approval

The read-only assembly job uploads a single `release-payload-<tag>-<sha>` workflow artifact. The final
publisher job:

- depends on successful validation, both native builds, and assembly;
- runs in the protected GitHub `release` environment;
- receives `contents: write` only after owner approval;
- verifies `github.ref` is the exact tag and the tag resolves to the assembled commit;
- creates or reuses only a draft Release for that exact tag/commit;
- uploads the exact payload assets, queries the remote asset set, and verifies names/non-zero sizes;
- publishes with the derived pre-release/stable state and immutable Chinese notes.

A retry may replace assets only on an unpublished draft for the same tag and commit. It refuses an
existing public Release, a draft targeting another commit, unexpected assets, or a tag moved after
assembly. This prevents duplicate Releases and cross-commit asset replacement.

## 8. Direct-Download-Only Boundary

V1 does not add `tauri-plugin-updater`, process restart capabilities, updater endpoints, public keys,
private-key Secrets, updater bundles, `.sig` files, or `latest.json`. Release scripts explicitly reject
these updater filenames so an unsigned metadata path cannot appear accidentally.

Future updater activation is a separate task requiring owner-generated key backup, committed public
key, private-key Secret, previous-version acceptance, tamper rejection, and key-rotation planning.

## 9. Script and Test Structure

Repository-owned Node scripts provide pure/testable release logic:

- exact changelog-section extraction;
- secret-set state classification from boolean presence only;
- provenance schema validation and cross-manifest consistency;
- deterministic filename/stable-vs-pre-release derivation;
- SHA-256 generation and sorted `SHA256SUMS`;
- Chinese release body/security table generation;
- exact asset-set validation and updater-file rejection.

Use Node's built-in test runner or a deterministic self-test command with fictional fixtures. Workflow
shell remains orchestration only; it must not duplicate release-contract parsing in Bash/PowerShell.

## 10. Compatibility, Rollback, and Operations

- No application database or IPC migration is introduced.
- Ordinary push CI behavior remains unchanged except for reusable release-contract checks.
- A failed tag run leaves diagnostic workflow artifacts and no public Release.
- A failed gated publisher may be retried only against the same unpublished draft/tag/commit.
- An incorrectly prepared tag is not moved; fix the version/changelog in a new commit and create a
  new tag according to owner policy.
- Certificate provisioning and the first real signed/notarized execution remain owner-controlled.

## 11. Key Trade-offs

- Universal macOS output increases build time and bundle size but avoids separate Intel/Apple Silicon
  Releases and metadata branches.
- Manual approval adds one release step but prevents an accidental tag from immediately becoming
  public.
- Unsigned/ad-hoc artifacts remain downloadable for testing, but automatic pre-release marking keeps
  them distinct from a fully signed stable distribution.
- Keeping updater activation out of V1 shortens the trust setup and removes private-key-loss risk from
  the first Release while preserving a strict future boundary.
