import fs from "node:fs";
import process from "node:process";
import { extractReleaseNotes, validateVersionTag } from "./release-contract.mjs";

const ref = process.argv[2] ?? process.env.GITHUB_REF_NAME ?? "";
let version;
try {
  version = validateVersionTag(ref);
} catch {
  console.error(`发布标签必须精确使用 v<major>.<minor>.<patch> 格式，收到：${ref || "（空）"}`);
  process.exit(1);
}
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

try {
  extractReleaseNotes(fs.readFileSync("CHANGELOG.zh-CN.md", "utf8"), version);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}

console.log(`发布标签、项目版本和更新日志一致：${ref}`);
