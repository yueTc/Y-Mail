//! 生成 Tauri sidecar：把 `mail-mcp` 的可执行文件按目标三元组后缀放进 `src-tauri/binaries/`。
//!
//! Tauri 2 约定：配置里写 `binaries/em-master-mcp`，磁盘上的文件名必须是
//! `em-master-mcp-<目标三元组>.exe`（Windows）。本脚本负责编译并按约定命名。
//!
//! 用法：
//!   node scripts/build-mcp-sidecar.mjs            # 给当前主机三元组出 release 版
//!   npm run build:mcp-sidecar                     # 同上（npm 脚本）
//!   node scripts/build-mcp-sidecar.mjs --debug     # 出 debug 版（`tauri build --debug` 用）
//!
//! `tauri build` 会把它挂到 `beforeBundleCommand` 上自动执行，所以正常打包不用手动跑。

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const rootDir = resolve(scriptDir, "..");

/** 找到 cargo；优先环境变量，其次 PATH，最后退回 ~/.cargo/bin。 */
function resolveCargo() {
  if (process.env.CARGO && existsSync(process.env.CARGO)) {
    return process.env.CARGO;
  }
  const home = process.env.USERPROFILE || process.env.HOME || "";
  const fallback = join(home, ".cargo", "bin", process.platform === "win32" ? "cargo.exe" : "cargo");
  if (home && existsSync(fallback)) {
    return fallback;
  }
  return "cargo";
}

/** 读当前 Rust 主机三元组。 */
function hostTriple() {
  const output = execFileSync(resolveCargo(), ["-vV"], { encoding: "utf8" });
  const line = output.split(/\r?\n/).find((item) => item.startsWith("host:"));
  if (!line) {
    throw new Error("读不到 rustc 主机三元组，请确认已安装 Rust");
  }
  return line.slice("host:".length).trim();
}

const debug = process.argv.includes("--debug") || process.env.TAURI_ENV_DEBUG === "true";
const host = hostTriple();
// `tauri build --target <三方三元组>` 时 CLI 会把目标三元组传进来；等于主机时不必单开 target 目录。
const requested = (process.env.TAURI_ENV_TARGET_TRIPLE || "").trim();
const triple = requested || host;
const useTargetFlag = Boolean(requested) && requested !== host;
const profile = debug ? "debug" : "release";

const cargoArgs = ["build", "-p", "mail-mcp", "--bin", "em-master-mcp"];
if (!debug) {
  cargoArgs.push("--release");
}
if (useTargetFlag) {
  cargoArgs.push("--target", triple);
}

console.log(`[mcp-sidecar] cargo ${cargoArgs.join(" ")}`);
execFileSync(resolveCargo(), cargoArgs, { cwd: rootDir, stdio: "inherit" });

const targetRoot = process.env.CARGO_TARGET_DIR
  ? resolve(rootDir, process.env.CARGO_TARGET_DIR)
  : join(rootDir, "target");
const exeName = triple.includes("windows") ? "em-master-mcp.exe" : "em-master-mcp";
const builtPath = useTargetFlag
  ? join(targetRoot, triple, profile, exeName)
  : join(targetRoot, profile, exeName);

if (!existsSync(builtPath)) {
  throw new Error(`编译产物不存在：${builtPath}`);
}

const outDir = join(rootDir, "src-tauri", "binaries");
mkdirSync(outDir, { recursive: true });
const outName = triple.includes("windows")
  ? `em-master-mcp-${triple}.exe`
  : `em-master-mcp-${triple}`;
const outPath = join(outDir, outName);
copyFileSync(builtPath, outPath);
console.log(`[mcp-sidecar] 已生成 ${outPath}`);