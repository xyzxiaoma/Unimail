import { spawnSync } from "node:child_process";
import fs from "node:fs";
import process from "node:process";

const BINDINGS = "src/lib/ipc/bindings.ts";
const before = fs.readFileSync(BINDINGS, "utf8");
const generated = spawnSync("cargo", ["run", "-p", "unimail-core", "--bin", "export-bindings"], {
  stdio: "inherit",
});

if (generated.error) {
  console.error(`无法运行 IPC 绑定生成器：${generated.error.message}`);
  process.exit(1);
}
if (generated.status !== 0) process.exit(generated.status ?? 1);

const after = fs.readFileSync(BINDINGS, "utf8");
if (before !== after) {
  console.error("IPC 绑定已过期；生成器已更新 src/lib/ipc/bindings.ts，请检查并提交该文件。");
  process.exit(1);
}

console.log("IPC 绑定与 Rust DTO 一致。");
