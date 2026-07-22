import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const root = new URL("../", import.meta.url);
const allowedPermissions = [
  "core:event:allow-listen",
  "core:event:allow-unlisten",
  "core:window:allow-destroy",
];
const allowedProductionLicenses = new Set(["(MPL-2.0 OR Apache-2.0)", "Apache-2.0 OR MIT", "MIT"]);
const secretPatterns = [
  /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/u,
  /\bgithub_pat_[A-Za-z0-9_]{40,}\b/u,
  /\bgh[pousr]_[A-Za-z0-9]{30,}\b/u,
  /\bAKIA[A-Z0-9]{16}\b/u,
];
const forbiddenDiagnosticFields =
  /\b(?:account_id|message_id|operation_id|address|display_name|subject|body|recipient|search_query|cursor|token|credential_ref|database_path|cache_path|destination_path|hostname|environment)\b/u;

function fail(message) {
  console.error(message);
  process.exitCode = 1;
}

function readJson(path) {
  return JSON.parse(readFileSync(new URL(path, root), "utf8"));
}

function validateCapabilities(capability) {
  const permissions = capability.permissions;
  if (!Array.isArray(permissions) || permissions.some((value) => typeof value !== "string")) {
    return "Tauri capability permissions 必须是字符串数组。";
  }
  if (JSON.stringify(permissions) !== JSON.stringify(allowedPermissions)) {
    return `主窗口权限必须精确为：${allowedPermissions.join(", ")}`;
  }
  return null;
}

function validateCsp(config) {
  const csp = config.app?.security?.csp;
  if (typeof csp !== "string") return "Tauri CSP 必须是非空字符串。";
  const forbidden = ["*", "'unsafe-eval'", "https:", "http:", "ws:", "wss:"];
  for (const value of forbidden) {
    if (csp.split(/\s+/u).includes(value)) return `Tauri CSP 不得包含 ${value}。`;
  }
  const required = [
    "default-src 'self'",
    "connect-src ipc: http://ipc.localhost",
    "script-src 'self'",
    "style-src 'self'",
    "object-src 'none'",
    "base-uri 'none'",
    "form-action 'none'",
  ];
  for (const directive of required) {
    if (!csp.includes(directive)) return `Tauri CSP 缺少：${directive}`;
  }
  if (csp.includes("'unsafe-inline'")) return "顶层 Tauri CSP 不得允许 unsafe-inline。";
  return null;
}

function trackedFiles() {
  return execFileSync("git", ["ls-files", "-co", "--exclude-standard", "-z"], {
    cwd: root,
    encoding: "utf8",
  })
    .split("\0")
    .filter(Boolean);
}

function isText(buffer) {
  return buffer.length <= 1_500_000 && !buffer.subarray(0, 8192).includes(0);
}

function scanTrackedSecrets() {
  const excluded = new Set(["Cargo.lock", "package-lock.json", "scripts/check-security.mjs"]);
  for (const path of trackedFiles()) {
    if (excluded.has(path)) continue;
    const buffer = readFileSync(new URL(path.replaceAll("\\", "/"), root));
    if (!isText(buffer)) continue;
    const text = buffer.toString("utf8");
    for (const pattern of secretPatterns) {
      if (pattern.test(text)) fail(`检测到高置信秘密模式：${path}`);
    }
  }
}

function scanRuntimeOutput() {
  for (const path of trackedFiles()) {
    const normalized = path.replaceAll("\\", "/");
    if (normalized === "crates/unimail-core/src/bin/export-bindings.rs") continue;
    if (/^(?:src-tauri|crates\/[^/]+)\/src\/.*\.rs$/u.test(normalized)) {
      const text = readFileSync(new URL(normalized, root), "utf8");
      if (/\b(?:println|eprintln|dbg)!\s*\(/u.test(text)) {
        fail(`运行时代码不得输出调试信息：${normalized}`);
      }
    }
    if (/^src\/.*\.[cm]?[jt]sx?$/u.test(normalized)) {
      const text = readFileSync(new URL(normalized, root), "utf8");
      if (/\bconsole\.(?:debug|error|info|log|trace|warn)\s*\(/u.test(text)) {
        fail(`前端运行时代码不得输出控制台信息：${normalized}`);
      }
    }
  }
}

function checkProductionNpmLicenses() {
  const lock = readJson("package-lock.json");
  for (const [path, value] of Object.entries(lock.packages ?? {})) {
    if (!path || value.dev === true) continue;
    if (!allowedProductionLicenses.has(value.license)) {
      fail(`生产 npm 依赖许可证未审阅：${path} (${value.license ?? "缺失"})`);
    }
  }
}

function scanDiagnosticFiles() {
  for (const path of [
    "crates/unimail-core/src/security.rs",
    "src/lib/ipc/security-diagnostics.ts",
  ]) {
    try {
      const text = readFileSync(new URL(path, root), "utf8");
      if (forbiddenDiagnosticFields.test(text)) fail(`安全诊断包含禁止字段名：${path}`);
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
  }
}

function selfTest() {
  const badCapability = validateCapabilities({ permissions: ["core:default"] });
  const goodCapability = validateCapabilities({ permissions: allowedPermissions });
  const badCsp = validateCsp({
    app: { security: { csp: "default-src *; script-src 'self' 'unsafe-eval'" } },
  });
  if (!badCapability || goodCapability !== null || !badCsp) {
    throw new Error("安全检查自测失败。");
  }
  console.log("安全检查自测通过。");
}

if (process.argv.includes("--self-test")) {
  selfTest();
} else {
  const capabilityError = validateCapabilities(readJson("src-tauri/capabilities/default.json"));
  if (capabilityError) fail(capabilityError);
  const cspError = validateCsp(readJson("src-tauri/tauri.conf.json"));
  if (cspError) fail(cspError);
  scanTrackedSecrets();
  scanRuntimeOutput();
  scanDiagnosticFiles();
  checkProductionNpmLicenses();
  if (process.exitCode === undefined) console.log("安全策略、秘密、输出与许可证检查通过。");
}
