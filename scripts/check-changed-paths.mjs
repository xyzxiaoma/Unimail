import process from "node:process";
import { changedFiles } from "./git-changes.mjs";

const FORBIDDEN = [
  { pattern: /(?:^|\/)node_modules\//u, reason: "Node 依赖目录" },
  { pattern: /(?:^|\/)target\//u, reason: "Rust/Tauri 构建目录" },
  {
    pattern:
      /(?:^|\/)(?:dist|coverage|playwright-report|test-results|blob-report|maildata|cache|logs|secrets|\.secrets)\//u,
    reason: "生成、本地数据或密钥目录",
  },
  { pattern: /(?:^|\/)\.env(?:\..+)?$/u, reason: "环境变量文件" },
  {
    pattern: /\.(?:db|sqlite|sqlite3)(?:-.+)?$|\.(?:eml|mbox)$/iu,
    reason: "本地邮件或数据库文件",
  },
  {
    pattern:
      /\.(?:pem|key|p8|der|pvk|p12|pfx|cer|crt|mobileprovision|provisionprofile|jks|keystore|asc|gpg|sig|secret|secrets)$/iu,
    reason: "密钥、证书或签名材料",
  },
];

const allowed = new Set([".env.example"]);
const violations = changedFiles().flatMap((file) => {
  if (allowed.has(file)) return [];
  const rule = FORBIDDEN.find(({ pattern }) => pattern.test(file));
  return rule ? [{ file, reason: rule.reason }] : [];
});

if (violations.length > 0) {
  console.error("检测到禁止提交的路径：");
  for (const { file, reason } of violations) console.error(`- ${file}（${reason}）`);
  process.exit(1);
}

console.log("变更路径检查通过。");
