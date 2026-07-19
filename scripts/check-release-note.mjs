import fs from "node:fs";
import process from "node:process";
import { changedFiles } from "./git-changes.mjs";

const CHANGELOG = "CHANGELOG.zh-CN.md";
const USER_VISIBLE = [
  /^index\.html$/u,
  /^src\//u,
  /^src-tauri\/(?:src|capabilities|icons|tauri\.conf\.json)/u,
  /^crates\//u,
  /^public\//u,
  /^package\.json$/u,
  /^\.github\/workflows\//u,
];
const EXCLUDED = [
  /(?:^|\/)(?:__tests__|test|tests|fixtures)\//u,
  /\.(?:test|spec)\.[cm]?[jt]sx?$/u,
  /^src\/(?:bindings\/|lib\/ipc\/bindings\.ts$)/u,
];

const files = changedFiles();
const visible = files
  .filter((file) => USER_VISIBLE.some((pattern) => pattern.test(file)))
  .filter((file) => !EXCLUDED.some((pattern) => pattern.test(file)));

if (visible.length === 0) {
  console.log("未检测到需要发布说明的用户可见变更。");
  process.exit(0);
}

if (!files.includes(CHANGELOG)) {
  console.error("检测到用户可见变更，但本次变更未包含 CHANGELOG.zh-CN.md：");
  for (const file of visible) console.error(`- ${file}`);
  process.exit(1);
}

const changelog = fs.readFileSync(CHANGELOG, "utf8");
const lines = changelog.split(/\r?\n/u);
const start = lines.findIndex((line) => line.trim() === "## 未发布");
const end =
  start === -1 ? -1 : lines.findIndex((line, index) => index > start && /^##\s+/u.test(line));
const unreleased =
  start === -1 ? "" : lines.slice(start + 1, end === -1 ? undefined : end).join("\n");
const hasNote = unreleased
  .split(/\r?\n/u)
  .some((line) => /^-\s+(?!暂无[。.]?$).+/u.test(line.trim()));

if (!hasNote) {
  console.error("CHANGELOG.zh-CN.md 的“未发布”章节必须包含至少一条非“暂无”的中文发布说明。");
  process.exit(1);
}

console.log(`发布说明检查通过（${visible.length} 个用户可见文件）。`);
