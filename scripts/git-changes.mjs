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
  const configuredBase =
    positional[0] ?? process.env.UNIMAIL_BASE_SHA ?? process.env.GITHUB_EVENT_BEFORE;
  const configuredHead = positional[1] ?? process.env.UNIMAIL_HEAD_SHA ?? process.env.GITHUB_SHA;
  const head = configuredHead ?? "HEAD";

  if (refExists(configuredBase)) {
    return lines(git(["diff", "--name-only", "--diff-filter=ACMR", configuredBase, head])).map(
      normalizePath,
    );
  }

  if (configuredBase === undefined && configuredHead === undefined && refExists("HEAD")) {
    const workingTree = lines(git(["diff", "HEAD", "--name-only", "--diff-filter=ACMR"]));
    const untracked = lines(git(["ls-files", "--others", "--exclude-standard"]));
    return [...new Set([...workingTree, ...untracked].map(normalizePath))];
  }

  if (refExists(head)) return rootFiles(head).map(normalizePath);

  return lines(git(["ls-files", "--cached", "--others", "--exclude-standard"])).map(normalizePath);
}
