import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  assembleReleasePayload,
  classifySecretPresence,
  extractReleaseNotes,
  stagePlatformArtifact,
  validateVersionTag,
  verifyReleasePayload,
} from "./release-contract.mjs";

function expectFailure(action, pattern) {
  assert.throws(action, pattern);
}

function checkWorkflowContract() {
  const workflow = fs.readFileSync(".github/workflows/release.yml", "utf8");
  assert.match(workflow, /workflow_dispatch:/u);
  assert.match(workflow, /tags:\s*\n\s*- ["']v\*["']/u);
  assert.match(workflow, /permissions:\s*\n\s*contents: read/u);
  assert.match(workflow, /environment: release/u);
  assert.match(workflow, /github\.event_name == 'push'.*refs\/tags\/v/u);
  assert.match(workflow, /universal-apple-darwin/u);
  assert.match(workflow, /--bundles nsis/u);
  assert.match(workflow, /if: always\(\)/u);
  assert.doesNotMatch(workflow, /ENABLE_DRAFT_RELEASE/u);
  assert.doesNotMatch(workflow, /release-contract\.mjs[^\r\n]*\+\s+--/u);
  assert.equal((workflow.match(/contents: write/gu) ?? []).length, 1);

  const usesLines = workflow
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .filter((line) => /^-?\s*uses:/u.test(line));
  assert.ok(usesLines.length > 0);
  for (const line of usesLines) assert.match(line, /@[0-9a-f]{40}(?:\s+#.*)?$/u);
}

function checkUpdaterBoundary() {
  const files = [
    "package.json",
    "Cargo.toml",
    "src-tauri/Cargo.toml",
    "src-tauri/tauri.conf.json",
    "src-tauri/capabilities/default.json",
  ];
  const combined = files.map((file) => fs.readFileSync(file, "utf8")).join("\n");
  assert.doesNotMatch(
    combined,
    /tauri-plugin-updater|tauri_plugin_updater|createUpdaterArtifacts|plugins\s*[.:]\s*updater/iu,
  );
}

function fixtureChangelog() {
  return `# 更新日志\n\n## 未发布\n\n### 新增\n\n- 下一版。\n\n## 0.1.0 - 2026-07-22\n\n### 新增\n\n- 可验证的测试发布。\n`;
}

function writeFixture(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, value);
}

function makeManifestFixtures(root, signing) {
  const tag = "v0.1.0";
  const version = "0.1.0";
  const commit = "a".repeat(40);
  const windowsSource = path.join(root, "source", "installer.exe");
  const macSource = path.join(root, "source", "installer.dmg");
  writeFixture(windowsSource, "fictional windows installer");
  writeFixture(macSource, "fictional macos installer");
  stagePlatformArtifact({
    platform: "windows",
    installer: windowsSource,
    outputDir: path.join(root, "input", "windows"),
    version,
    tag,
    commit,
    platformSigning: signing ? "authenticode" : "unsigned",
    signingIdentity: signing ? "CN=Fictional Windows Publisher" : null,
    notarized: false,
    nativeStartupPassed: true,
  });
  stagePlatformArtifact({
    platform: "macos",
    installer: macSource,
    outputDir: path.join(root, "input", "macos"),
    version,
    tag,
    commit,
    platformSigning: signing ? "developer-id" : "adhoc",
    signingIdentity: signing ? "Developer ID Application: Fictional Publisher (ABCDE12345)" : null,
    notarized: signing,
    nativeStartupPassed: true,
  });
  const changelog = path.join(root, "CHANGELOG.zh-CN.md");
  writeFixture(changelog, fixtureChangelog());
  return { tag, version, commit, changelog };
}

function expectAssemblyFailure(mutator, pattern) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "unimail-release-invalid-"));
  try {
    const identity = makeManifestFixtures(root, false);
    mutator(root);
    expectFailure(
      () =>
        assembleReleasePayload({
          inputDir: path.join(root, "input"),
          outputDir: path.join(root, "payload"),
          version: identity.version,
          tag: identity.tag,
          commit: identity.commit,
          changelogFile: identity.changelog,
        }),
      pattern,
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

function selfTest() {
  assert.equal(validateVersionTag("v0.1.0"), "0.1.0");
  expectFailure(() => validateVersionTag("v0.1.0-rc.1"), /vX\.Y\.Z/u);
  assert.match(extractReleaseNotes(fixtureChangelog(), "0.1.0"), /可验证/u);
  expectFailure(
    () => extractReleaseNotes("## 0.1.0 - 2026-07-22\n\n- 暂无。\n", "0.1.0"),
    /没有实际发布说明/u,
  );

  const names = ["CERTIFICATE", "PASSWORD"];
  assert.deepEqual(classifySecretPresence(names, {}), { state: "absent", missing: names });
  assert.deepEqual(classifySecretPresence(names, { CERTIFICATE: true, PASSWORD: true }), {
    state: "complete",
    missing: [],
  });
  assert.deepEqual(classifySecretPresence(names, { CERTIFICATE: true }), {
    state: "partial",
    missing: ["PASSWORD"],
  });

  for (const signing of [false, true]) {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "unimail-release-"));
    try {
      const identity = makeManifestFixtures(root, signing);
      const outputDir = path.join(root, "payload");
      const meta = assembleReleasePayload({
        inputDir: path.join(root, "input"),
        outputDir,
        version: identity.version,
        tag: identity.tag,
        commit: identity.commit,
        changelogFile: identity.changelog,
      });
      assert.equal(meta.prerelease, !signing);
      assert.equal(meta.expectedAssets.length, 5);
      assert.ok(meta.expectedAssets.every((asset) => !asset.name.endsWith(".sig")));
      verifyReleasePayload({ payloadDir: outputDir, ...identity });

      fs.appendFileSync(path.join(outputDir, meta.expectedAssets[0].name), "tampered");
      expectFailure(
        () => verifyReleasePayload({ payloadDir: outputDir, ...identity }),
        /大小不一致|校验和不一致/u,
      );
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  }

  const missingPlatformRoot = fs.mkdtempSync(path.join(os.tmpdir(), "unimail-release-missing-"));
  try {
    const identity = makeManifestFixtures(missingPlatformRoot, false);
    fs.rmSync(path.join(missingPlatformRoot, "input", "macos"), { recursive: true, force: true });
    expectFailure(
      () =>
        assembleReleasePayload({
          inputDir: path.join(missingPlatformRoot, "input"),
          outputDir: path.join(missingPlatformRoot, "payload"),
          version: identity.version,
          tag: identity.tag,
          commit: identity.commit,
          changelogFile: identity.changelog,
        }),
      /恰好找到 2 份/u,
    );
  } finally {
    fs.rmSync(missingPlatformRoot, { recursive: true, force: true });
  }

  expectAssemblyFailure((root) => {
    const manifestFile = path.join(root, "input", "macos", "build-provenance.json");
    const manifest = JSON.parse(fs.readFileSync(manifestFile, "utf8"));
    manifest.commit = "b".repeat(40);
    fs.writeFileSync(manifestFile, JSON.stringify(manifest));
  }, /commit 与发布请求不一致/u);

  expectAssemblyFailure((root) => {
    const manifestFile = path.join(root, "input", "windows", "build-provenance.json");
    const manifest = JSON.parse(fs.readFileSync(manifestFile, "utf8"));
    manifest.nativeStartupPassed = false;
    fs.writeFileSync(manifestFile, JSON.stringify(manifest));
  }, /启动冒烟未通过/u);

  expectAssemblyFailure((root) => {
    writeFixture(path.join(root, "input", "windows", "unexpected.txt"), "unexpected");
  }, /文件集合无效/u);

  expectAssemblyFailure((root) => {
    writeFixture(path.join(root, "input", "latest.json"), "{}");
  }, /禁止 updater 资产/u);

  const updaterRoot = fs.mkdtempSync(path.join(os.tmpdir(), "unimail-release-updater-"));
  try {
    const updater = path.join(updaterRoot, "Unimail.app.tar.gz.sig");
    writeFixture(updater, "fictional signature");
    expectFailure(
      () =>
        stagePlatformArtifact({
          platform: "macos",
          installer: updater,
          outputDir: path.join(updaterRoot, "output"),
          version: "0.1.0",
          tag: "v0.1.0",
          commit: "a".repeat(40),
          platformSigning: "adhoc",
          signingIdentity: null,
          notarized: false,
          nativeStartupPassed: true,
        }),
      /禁止 updater 资产/u,
    );
  } finally {
    fs.rmSync(updaterRoot, { recursive: true, force: true });
  }
}

try {
  selfTest();
  checkWorkflowContract();
  checkUpdaterBoundary();
  console.log("发布契约、工作流边界与直链下载策略检查通过。");
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
