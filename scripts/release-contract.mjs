import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

const TAG_PATTERN = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u;
const COMMIT_PATTERN = /^[0-9a-f]{40}$/u;
const FORBIDDEN_ASSET_PATTERNS = [
  /(?:^|[/\\])latest\.json$/iu,
  /\.sig$/iu,
  /\.app\.tar\.gz$/iu,
  /\.app\.tar$/iu,
  /\.nsis\.zip$/iu,
  /\.msi\.zip$/iu,
];

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function normalizedBoolean(value, name) {
  if (value === true || value === "true") return true;
  if (value === false || value === "false") return false;
  throw new Error(`${name} 必须是 true 或 false。`);
}

function validateSafeIdentity(value) {
  if (value === undefined || value === null || value === "") return null;
  assert(typeof value === "string", "签名身份必须是字符串。");
  const identity = value.trim();
  assert(identity.length > 0 && identity.length <= 300, "签名身份长度无效。");
  assert(!/[\r\n\0]/u.test(identity), "签名身份包含非法控制字符。");
  return identity;
}

export function validateVersionTag(tag) {
  assert(typeof tag === "string" && TAG_PATTERN.test(tag), "发布标签必须精确使用 vX.Y.Z 格式。");
  return tag.slice(1);
}

export function extractReleaseNotes(changelog, version) {
  assert(typeof changelog === "string", "更新日志必须是文本。");
  assert(/^\d+\.\d+\.\d+$/u.test(version), "发布版本必须使用 X.Y.Z 格式。");

  const escaped = version.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
  const heading = new RegExp(`^## \\[?${escaped}\\]?\\s+-\\s+\\d{4}-\\d{2}-\\d{2}\\s*$`, "u");
  const lines = changelog.split(/\r?\n/u);
  const start = lines.findIndex((line) => heading.test(line));
  assert(start !== -1, `CHANGELOG.zh-CN.md 缺少版本 ${version} 的正式章节。`);

  const nextHeading = lines.findIndex((line, index) => index > start && /^##\s+/u.test(line));
  const notes = lines
    .slice(start + 1, nextHeading === -1 ? undefined : nextHeading)
    .join("\n")
    .trim();
  assert(notes.length > 0, `CHANGELOG.zh-CN.md 的版本 ${version} 章节为空。`);

  const meaningfulBullet = notes
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .find(
      (line) =>
        /^-\s+.+/u.test(line) &&
        !/^-\s*(?:暂无[。.]?|待定[。.]?|TBD|TODO|占位(?:内容)?[。.]?)$/iu.test(line),
    );
  assert(meaningfulBullet, `CHANGELOG.zh-CN.md 的版本 ${version} 章节没有实际发布说明。`);
  assert(!/(?:TODO|TBD|待补充|占位内容)/iu.test(notes), `版本 ${version} 的发布说明仍含占位文本。`);
  return notes;
}

export function classifySecretPresence(names, presence) {
  assert(Array.isArray(names) && names.length > 0, "必须提供 Secret 名称列表。");
  const states = names.map((name) => Boolean(presence[name]));
  if (states.every(Boolean)) return { state: "complete", missing: [] };
  if (states.every((value) => !value)) return { state: "absent", missing: [...names] };
  return { state: "partial", missing: names.filter((name) => !presence[name]) };
}

export function installerFileName({ platform, version, platformSigning, notarized }) {
  if (platform === "windows") {
    assert(["unsigned", "authenticode"].includes(platformSigning), "Windows 签名状态无效。");
    assert(notarized === false, "Windows 安装包不得标记为已公证。");
    return `Unimail_${version}_windows_x86_64_${platformSigning}.exe`;
  }
  if (platform === "macos") {
    assert(["adhoc", "developer-id"].includes(platformSigning), "macOS 签名状态无效。");
    assert(
      (platformSigning === "developer-id" && notarized === true) ||
        (platformSigning === "adhoc" && notarized === false),
      "macOS Developer ID 必须同时完成公证，ad-hoc 包不得标记为已公证。",
    );
    const trust =
      platformSigning === "developer-id" ? "developer-id-notarized" : "adhoc-not-notarized";
    return `Unimail_${version}_macos_universal_${trust}.dmg`;
  }
  throw new Error(`不支持的发布平台：${platform}`);
}

export function validateProvenance(manifest, expected = {}) {
  assert(
    manifest && typeof manifest === "object" && !Array.isArray(manifest),
    "provenance 必须是对象。",
  );
  assert(manifest.schemaVersion === 1, "provenance schemaVersion 必须为 1。");
  const version = validateVersionTag(manifest.tag);
  assert(manifest.version === version, "provenance 版本与标签不一致。");
  assert(COMMIT_PATTERN.test(manifest.commit), "provenance commit 必须是 40 位小写十六进制 SHA。");
  assert(manifest.nativeStartupPassed === true, "原生启动冒烟未通过。");
  assert(typeof manifest.notarized === "boolean", "notarized 必须是布尔值。");

  if (manifest.platform === "windows") {
    assert(manifest.architecture === "x86_64", "Windows 架构必须是 x86_64。");
    assert(manifest.installerKind === "nsis", "Windows 安装包必须是 NSIS。");
  } else if (manifest.platform === "macos") {
    assert(manifest.architecture === "universal", "macOS 架构必须是 universal。");
    assert(manifest.installerKind === "dmg", "macOS 安装包必须是 DMG。");
  } else {
    throw new Error(`provenance 平台无效：${manifest.platform ?? "缺失"}`);
  }

  const identity = validateSafeIdentity(manifest.signingIdentity);
  if (["authenticode", "developer-id"].includes(manifest.platformSigning)) {
    assert(identity, "生产签名 provenance 必须记录签名身份。");
  } else {
    assert(identity === null, "未签名或 ad-hoc provenance 不得声称签名身份。");
  }

  const expectedName = installerFileName(manifest);
  assert(manifest.installerFile === expectedName, `安装包文件名必须为 ${expectedName}。`);
  if (expected.version)
    assert(manifest.version === expected.version, "provenance 版本与发布请求不一致。");
  if (expected.tag) assert(manifest.tag === expected.tag, "provenance 标签与发布请求不一致。");
  if (expected.commit)
    assert(manifest.commit === expected.commit, "provenance commit 与发布请求不一致。");
  if (expected.platform)
    assert(manifest.platform === expected.platform, "provenance 平台与预期不一致。");
  return { ...manifest, signingIdentity: identity };
}

export function derivePrerelease(manifests) {
  const windows = manifests.find((item) => item.platform === "windows");
  const macos = manifests.find((item) => item.platform === "macos");
  assert(windows && macos, "必须同时包含 Windows 与 macOS provenance。");
  return !(
    windows.platformSigning === "authenticode" &&
    macos.platformSigning === "developer-id" &&
    macos.notarized === true
  );
}

export function sha256File(file) {
  const hash = createHash("sha256");
  hash.update(fs.readFileSync(file));
  return hash.digest("hex");
}

function rejectForbiddenAsset(file) {
  const normalized = file.replaceAll("\\", "/");
  const forbidden = FORBIDDEN_ASSET_PATTERNS.find((pattern) => pattern.test(normalized));
  assert(!forbidden, `V1 直链发布禁止 updater 资产：${path.basename(file)}`);
}

function listFilesRecursively(root) {
  const files = [];
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const full = path.join(root, entry.name);
    if (entry.isDirectory()) files.push(...listFilesRecursively(full));
    else if (entry.isFile()) files.push(full);
  }
  return files;
}

export function stagePlatformArtifact({
  platform,
  installer,
  outputDir,
  version,
  tag,
  commit,
  platformSigning,
  signingIdentity,
  notarized,
  nativeStartupPassed,
}) {
  validateVersionTag(tag);
  assert(version === tag.slice(1), "暂存版本与标签不一致。");
  assert(COMMIT_PATTERN.test(commit), "暂存 commit 必须是 40 位小写十六进制 SHA。");
  assert(fs.existsSync(installer) && fs.statSync(installer).isFile(), "没有找到待暂存的安装包。");
  assert(fs.statSync(installer).size > 0, "待暂存安装包为空。");
  rejectForbiddenAsset(installer);

  const normalizedNotarized = normalizedBoolean(notarized, "notarized");
  const normalizedStartupPassed = normalizedBoolean(nativeStartupPassed, "nativeStartupPassed");
  const manifest = validateProvenance({
    schemaVersion: 1,
    platform,
    version,
    tag,
    commit,
    architecture: platform === "windows" ? "x86_64" : "universal",
    installerKind: platform === "windows" ? "nsis" : "dmg",
    installerFile: installerFileName({
      platform,
      version,
      platformSigning,
      notarized: normalizedNotarized,
    }),
    platformSigning,
    signingIdentity: validateSafeIdentity(signingIdentity),
    notarized: normalizedNotarized,
    nativeStartupPassed: normalizedStartupPassed,
  });

  fs.rmSync(outputDir, { recursive: true, force: true });
  fs.mkdirSync(outputDir, { recursive: true });
  fs.copyFileSync(installer, path.join(outputDir, manifest.installerFile));
  writeJson(path.join(outputDir, "build-provenance.json"), manifest);
  return manifest;
}

function signingDescription(manifest) {
  switch (manifest.platformSigning) {
    case "authenticode":
      return `Authenticode 已验证（${manifest.signingIdentity}）`;
    case "developer-id":
      return `Developer ID 已验证（${manifest.signingIdentity}）`;
    case "adhoc":
      return "ad-hoc（测试包）";
    case "unsigned":
      return "未签名（测试包）";
    default:
      throw new Error("未知签名状态。");
  }
}

export function renderReleaseBody({ notes, manifests, installerHashes, prerelease }) {
  const windows = manifests.find((item) => item.platform === "windows");
  const macos = manifests.find((item) => item.platform === "macos");
  const warning = prerelease
    ? "> ⚠️ 此版本至少有一个平台未完成生产签名或公证，仅作为测试预发布版。安装时可能出现系统安全提示，请先核对 SHA-256。\n\n"
    : "";
  const rows = [windows, macos]
    .map((manifest) => {
      const platformName = manifest.platform === "windows" ? "Windows" : "macOS";
      const notarization =
        manifest.platform === "macos" ? (manifest.notarized ? "已验证" : "未公证") : "不适用";
      return `| ${platformName} | ${manifest.architecture} | \`${manifest.installerFile}\` | ${signingDescription(manifest)} | ${notarization} | \`${installerHashes[manifest.installerFile]}\` |`;
    })
    .join("\n");

  return `${notes}\n\n---\n\n${warning}## 安装包与安全状态\n\n| 平台 | 架构 | 文件 | 平台签名 | Apple 公证 | SHA-256 |\n| --- | --- | --- | --- | --- | --- |\n${rows}\n\n- 应用内更新：未启用；V1 仅支持从 GitHub Release 手动下载。\n- 完整校验值见 \`SHA256SUMS\`，构建来源见 \`release-provenance.json\`。\n`;
}

export function assembleReleasePayload({
  inputDir,
  outputDir,
  version,
  tag,
  commit,
  changelogFile,
}) {
  validateVersionTag(tag);
  assert(version === tag.slice(1), "组装版本与标签不一致。");
  assert(COMMIT_PATTERN.test(commit), "组装 commit 必须是 40 位小写十六进制 SHA。");
  assert(fs.existsSync(inputDir), "发布候选输入目录不存在。");

  const allInputFiles = listFilesRecursively(inputDir);
  for (const file of allInputFiles) rejectForbiddenAsset(file);
  const manifestFiles = allInputFiles.filter(
    (file) => path.basename(file) === "build-provenance.json",
  );
  assert(
    manifestFiles.length === 2,
    `必须恰好找到 2 份平台 provenance，实际为 ${manifestFiles.length}。`,
  );

  const manifests = manifestFiles.map((file) => {
    const manifest = validateProvenance(readJson(file), { version, tag, commit });
    const directoryFiles = fs.readdirSync(path.dirname(file), { withFileTypes: true });
    assert(
      directoryFiles.every((entry) => entry.isFile()),
      "平台候选 artifact 不得包含嵌套目录。",
    );
    const names = directoryFiles.map((entry) => entry.name).sort();
    assert(
      JSON.stringify(names) ===
        JSON.stringify(["build-provenance.json", manifest.installerFile].sort()),
      `平台候选 artifact 文件集合无效：${manifest.platform}`,
    );
    const installerPath = path.join(path.dirname(file), manifest.installerFile);
    assert(fs.statSync(installerPath).size > 0, `安装包为空：${manifest.installerFile}`);
    return { manifest, installerPath };
  });

  const platforms = manifests.map(({ manifest }) => manifest.platform).sort();
  assert(
    JSON.stringify(platforms) === JSON.stringify(["macos", "windows"]),
    "平台集合必须精确为 Windows 与 macOS。",
  );
  const normalizedManifests = manifests
    .map(({ manifest }) => manifest)
    .sort((a, b) => a.platform.localeCompare(b.platform));
  const prerelease = derivePrerelease(normalizedManifests);
  const notes = extractReleaseNotes(fs.readFileSync(changelogFile, "utf8"), version);

  fs.rmSync(outputDir, { recursive: true, force: true });
  fs.mkdirSync(outputDir, { recursive: true });
  for (const { manifest, installerPath } of manifests) {
    fs.copyFileSync(installerPath, path.join(outputDir, manifest.installerFile));
  }

  const installerHashes = Object.fromEntries(
    normalizedManifests.map((manifest) => [
      manifest.installerFile,
      sha256File(path.join(outputDir, manifest.installerFile)),
    ]),
  );
  const provenance = {
    schemaVersion: 1,
    version,
    tag,
    commit,
    prerelease,
    updaterEnabled: false,
    platforms: normalizedManifests,
    assets: normalizedManifests.map((manifest) => ({
      file: manifest.installerFile,
      sha256: installerHashes[manifest.installerFile],
    })),
  };
  writeJson(path.join(outputDir, "release-provenance.json"), provenance);
  fs.writeFileSync(
    path.join(outputDir, "release-notes.zh-CN.md"),
    renderReleaseBody({ notes, manifests: normalizedManifests, installerHashes, prerelease }),
    "utf8",
  );

  const checksumNames = [
    ...normalizedManifests.map((manifest) => manifest.installerFile),
    "release-notes.zh-CN.md",
    "release-provenance.json",
  ].sort();
  const checksumLines = checksumNames.map(
    (name) => `${sha256File(path.join(outputDir, name))}  ${name}`,
  );
  fs.writeFileSync(path.join(outputDir, "SHA256SUMS"), `${checksumLines.join("\n")}\n`, "utf8");

  const publicAssetNames = [...checksumNames, "SHA256SUMS"].sort();
  const expectedAssets = publicAssetNames.map((name) => {
    const file = path.join(outputDir, name);
    return { name, size: fs.statSync(file).size, sha256: sha256File(file) };
  });
  const meta = { schemaVersion: 1, version, tag, commit, prerelease, expectedAssets };
  writeJson(path.join(outputDir, "release-meta.json"), meta);
  verifyReleasePayload({ payloadDir: outputDir, version, tag, commit });
  return meta;
}

export function verifyReleasePayload({ payloadDir, version, tag, commit }) {
  const metaFile = path.join(payloadDir, "release-meta.json");
  assert(fs.existsSync(metaFile), "发布 payload 缺少 release-meta.json。");
  const meta = readJson(metaFile);
  assert(meta.schemaVersion === 1, "release-meta schemaVersion 必须为 1。");
  assert(
    meta.version === version && meta.tag === tag && meta.commit === commit,
    "发布 payload 身份不匹配。",
  );
  assert(typeof meta.prerelease === "boolean", "release-meta prerelease 必须是布尔值。");
  assert(
    Array.isArray(meta.expectedAssets) && meta.expectedAssets.length === 5,
    "正式资产集合必须恰好包含 5 个文件。",
  );

  const expectedNames = meta.expectedAssets.map((asset) => asset.name).sort();
  const actualNames = fs
    .readdirSync(payloadDir, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name !== "release-meta.json")
    .map((entry) => entry.name)
    .sort();
  assert(
    JSON.stringify(actualNames) === JSON.stringify(expectedNames),
    "发布 payload 文件集合与清单不一致。",
  );
  for (const asset of meta.expectedAssets) {
    rejectForbiddenAsset(asset.name);
    const file = path.join(payloadDir, asset.name);
    assert(
      fs.statSync(file).size === asset.size && asset.size > 0,
      `发布资产大小不一致：${asset.name}`,
    );
    assert(sha256File(file) === asset.sha256, `发布资产校验和不一致：${asset.name}`);
  }
  return meta;
}

function requiredOption(values, name) {
  const value = values[name];
  assert(typeof value === "string" && value.length > 0, `缺少 --${name}。`);
  return value;
}

async function main() {
  const command = process.argv[2];
  const { values } = parseArgs({
    args: process.argv.slice(3),
    options: {
      version: { type: "string" },
      tag: { type: "string" },
      commit: { type: "string" },
      changelog: { type: "string" },
      output: { type: "string" },
      "input-dir": { type: "string" },
      "output-dir": { type: "string" },
      platform: { type: "string" },
      installer: { type: "string" },
      signing: { type: "string" },
      "signing-identity": { type: "string" },
      notarized: { type: "string" },
      "startup-passed": { type: "string" },
      "payload-dir": { type: "string" },
    },
    strict: true,
  });

  switch (command) {
    case "extract-notes": {
      const version = requiredOption(values, "version");
      const changelog = requiredOption(values, "changelog");
      const output = requiredOption(values, "output");
      fs.writeFileSync(
        output,
        `${extractReleaseNotes(fs.readFileSync(changelog, "utf8"), version)}\n`,
        "utf8",
      );
      break;
    }
    case "stage":
      stagePlatformArtifact({
        platform: requiredOption(values, "platform"),
        installer: requiredOption(values, "installer"),
        outputDir: requiredOption(values, "output-dir"),
        version: requiredOption(values, "version"),
        tag: requiredOption(values, "tag"),
        commit: requiredOption(values, "commit"),
        platformSigning: requiredOption(values, "signing"),
        signingIdentity: values["signing-identity"],
        notarized: requiredOption(values, "notarized"),
        nativeStartupPassed: requiredOption(values, "startup-passed"),
      });
      break;
    case "assemble":
      assembleReleasePayload({
        inputDir: requiredOption(values, "input-dir"),
        outputDir: requiredOption(values, "output-dir"),
        version: requiredOption(values, "version"),
        tag: requiredOption(values, "tag"),
        commit: requiredOption(values, "commit"),
        changelogFile: requiredOption(values, "changelog"),
      });
      break;
    case "verify-payload": {
      const meta = verifyReleasePayload({
        payloadDir: requiredOption(values, "payload-dir"),
        version: requiredOption(values, "version"),
        tag: requiredOption(values, "tag"),
        commit: requiredOption(values, "commit"),
      });
      process.stdout.write(`${JSON.stringify(meta)}\n`);
      break;
    }
    default:
      throw new Error("未知发布契约命令。支持：extract-notes、stage、assemble、verify-payload。");
  }
}

const executedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href;
if (executedDirectly) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  });
}
