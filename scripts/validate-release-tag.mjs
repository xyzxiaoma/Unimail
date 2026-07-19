import fs from "node:fs";
import process from "node:process";

const ref = process.argv[2] ?? process.env.GITHUB_REF_NAME ?? "";
const match = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?$/u.exec(ref);

if (!match) {
  console.error(`发布标签必须使用 v<major>.<minor>.<patch> 格式，收到：${ref || "（空）"}`);
  process.exit(1);
}

const version = ref.slice(1);
const packageJson = JSON.parse(fs.readFileSync("package.json", "utf8"));
const tauriConfig = JSON.parse(fs.readFileSync("src-tauri/tauri.conf.json", "utf8"));

function tomlSectionValue(file, section, key) {
  const lines = fs.readFileSync(file, "utf8").split(/\r?\n/u);
  let activeSection = "";
  for (const rawLine of lines) {
    const line = rawLine.replace(/\s+#.*$/u, "").trim();
    const heading = /^\[([^\]]+)\]$/u.exec(line);
    if (heading) {
      activeSection = heading[1];
      continue;
    }
    if (activeSection !== section) continue;
    const value = new RegExp(`^${key.replaceAll(".", "\\.")}\\s*=\\s*"([^"]+)"$`, "u").exec(
      line,
    )?.[1];
    if (value) return value;
  }
  return undefined;
}

const directCargoVersion = tomlSectionValue("src-tauri/Cargo.toml", "package", "version");
const cargoUsesWorkspaceVersion = fs
  .readFileSync("src-tauri/Cargo.toml", "utf8")
  .split(/\r?\n/u)
  .some((line) =>
    /^\s*(?:version\.workspace\s*=\s*true|version\s*=\s*\{\s*workspace\s*=\s*true\s*\})\s*(?:#.*)?$/u.test(
      line,
    ),
  );
const cargoVersion =
  directCargoVersion ??
  (cargoUsesWorkspaceVersion && fs.existsSync("Cargo.toml")
    ? tomlSectionValue("Cargo.toml", "workspace.package", "version")
    : undefined);

const versions = [
  ["package.json", packageJson.version],
  ["src-tauri/tauri.conf.json", tauriConfig.version],
  ["src-tauri/Cargo.toml", cargoVersion],
];
const mismatches = versions.filter(([, actual]) => actual !== version);

if (mismatches.length > 0) {
  console.error(`标签版本 ${version} 与项目版本不一致：`);
  for (const [file, actual] of mismatches) console.error(`- ${file}: ${actual ?? "缺失"}`);
  process.exit(1);
}

const changelog = fs.readFileSync("CHANGELOG.zh-CN.md", "utf8");
const escaped = version.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
const releaseHeading = new RegExp(`^## \\[?${escaped}\\]?\\s+-\\s+\\d{4}-\\d{2}-\\d{2}\\s*$`, "u");
const changelogLines = changelog.split(/\r?\n/u);
const releaseStart = changelogLines.findIndex((line) => releaseHeading.test(line));

if (releaseStart === -1) {
  console.error(
    `CHANGELOG.zh-CN.md 缺少版本 ${version} 的二级标题（例如“## ${version} - 2026-07-19”）。`,
  );
  process.exit(1);
}

const releaseEnd = changelogLines.findIndex(
  (line, index) => index > releaseStart && /^##\s+/u.test(line),
);
const releaseNotes = changelogLines.slice(
  releaseStart + 1,
  releaseEnd === -1 ? undefined : releaseEnd,
);
if (!releaseNotes.some((line) => /^-\s+(?!暂无[。.]?$).+/u.test(line.trim()))) {
  console.error(`CHANGELOG.zh-CN.md 的版本 ${version} 章节没有实际发布说明。`);
  process.exit(1);
}

console.log(`发布标签、项目版本和更新日志一致：${ref}`);
