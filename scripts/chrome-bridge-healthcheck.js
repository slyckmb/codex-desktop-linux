#!/usr/bin/env node

// Read-only Chrome bridge healthcheck.  It is intentionally independent of the
// Desktop UI so users and scheduled agents can run it after package installs.
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const args = new Set(process.argv.slice(2));
const json = args.has("--json");
const strict = args.has("--strict");
const runLive = !args.has("--no-live");
const home = process.env.HOME || "/home/michael";
const installRoot = process.env.CODEX_DESKTOP_ROOT || "/opt/codex-desktop";
const codexHome = process.env.CODEX_HOME || path.join(home, ".codex");
const pluginRoot = path.join(installRoot, "resources/plugins/openai-bundled/plugins/chrome");
const hostManifest = path.join(home, ".config/google-chrome/NativeMessagingHosts/com.openai.codexextension.json");
const extensionId = "hehggadaopoacecdllhhajmbjkdcmajg";

function check(name, ok, detail, severity = "error") {
  return { name, ok: Boolean(ok), detail, severity };
}

function command(script, extra = []) {
  const file = path.join(pluginRoot, "scripts", script);
  try {
    return JSON.parse(execFileSync(process.execPath, [file, ...extra], { encoding: "utf8" }));
  } catch (error) {
    return { error: error.message };
  }
}

function regularFile(file) {
  try { return fs.statSync(file).isFile(); } catch { return false; }
}

const checks = [];
checks.push(check("installed-plugin", regularFile(path.join(pluginRoot, ".codex-plugin/plugin.json")), pluginRoot));
checks.push(check("browser-client", regularFile(path.join(pluginRoot, "scripts/browser-client.mjs")), "bundled browser-client.mjs"));

let manifest;
try { manifest = JSON.parse(fs.readFileSync(hostManifest, "utf8")); } catch (error) { manifest = null; }
const origins = manifest?.allowed_origins || [];
checks.push(check("native-host-manifest", manifest?.name === "com.openai.codexextension" && origins.includes(`chrome-extension://${extensionId}/`), hostManifest));

const native = command("check-native-host-manifest.js", ["--browser", "chrome", "--json"]);
checks.push(check("native-host-diagnostic", native.correct === true, native.problem || "manifest diagnostic passed"));
const extension = command("check-extension-installed.js", ["--browser", "chrome", "--json"]);
checks.push(check("extension-installed", extension.installed === true && extension.enabled === true, extension.problem || `profile=${extension.selectedProfileDirectory || "unknown"}`));

const runtimeLink = path.join(codexHome, "plugins/linux-runtime-cache/openai-bundled/chrome/latest");
let runtimeTarget = null;
try { runtimeTarget = fs.realpathSync(runtimeLink); } catch {}
checks.push(check("managed-runtime", runtimeTarget !== null && regularFile(path.join(runtimeTarget, "extension-host/linux/x64/extension-host")), runtimeTarget || runtimeLink));

const failed = checks.filter((item) => !item.ok && item.severity === "error");
let liveBridge = { status: "disabled", detail: "Live probe disabled by --no-live." };
if (runLive) {
  try {
    liveBridge = JSON.parse(execFileSync(process.execPath, [path.join(__dirname, "chrome-bridge-live-probe.mjs")], { encoding: "utf8", timeout: 10000 }));
  } catch (error) {
    liveBridge = { status: "unavailable", detail: error.message };
  }
}
const result = { ok: failed.length === 0, checkedAt: new Date().toISOString(), installRoot, codexHome, checks, liveBridge };
// An ordinary shell has no Desktop-provided browser runtime. That is not a
// static installation failure; strict mode is for automation and requires proof.
if (strict && result.liveBridge.status !== "passed") result.ok = false;

if (json) process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
else {
  for (const item of checks) console.log(`${item.ok ? "PASS" : "FAIL"} ${item.name}: ${item.detail}`);
  console.log(`${result.ok ? "HEALTHY" : "UNHEALTHY"} Chrome bridge (live=${result.liveBridge.status})`);
}
process.exitCode = result.ok ? 0 : 1;
