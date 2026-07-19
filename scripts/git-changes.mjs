import { execFileSync, spawnSync } from "node:child_process";
import process from "node:process";

const ZERO_SHA = /^0+$/;

function git(args, options = {}) {
  return execFileSync("git", args, {
    cwd: process.cwd(),
    encoding: "utf8",
    stdio: [options.input === undefined ? "ignore" : "pipe", "pipe", "pipe"],
    ...options,
  }).trim();
}

function refExists(ref) {
  if (!ref || ZERO_SHA.test(ref)) return false;
  return (
    spawnSync("git", ["cat-file", "-e", `${ref}^{commit}`], {
      cwd: process.cwd(),
      stdio: "ignore",
    }).status === 0
  );
}

function rootFiles(head) {
  return lines(git(["diff-tree", "--root", "--no-commit-id", "--name-only", "-r", head]));
}

function lines(value) {
  return value
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .filter(Boolean);
}

export function normalizePath(file) {
  return file.replaceAll("\\", "/").replace(/^\.\//u, "");
}

export function changedFiles(argv = process.argv.slice(2)) {
  const filesIndex = argv.indexOf("--files");
  if (filesIndex !== -1) return argv.slice(filesIndex + 1).map(normalizePath);

  const staged = argv.includes("--staged");
  if (staged)
    return lines(git(["diff", "--cached", "--name-only", "--diff-filter=ACMR"])).map(normalizePath);

  const positional = argv.filter((arg) => !arg.startsWith("--"));
  const base = positional[0] ?? process.env.UNIMAIL_BASE_SHA ?? process.env.GITHUB_EVENT_BEFORE;
  const head = positional[1] ?? process.env.UNIMAIL_HEAD_SHA ?? process.env.GITHUB_SHA ?? "HEAD";

  if (refExists(base)) {
    return lines(git(["diff", "--name-only", "--diff-filter=ACMR", base, head])).map(normalizePath);
  }

  if (refExists(head)) return rootFiles(head).map(normalizePath);

  return lines(git(["ls-files", "--cached", "--others", "--exclude-standard"])).map(normalizePath);
}
