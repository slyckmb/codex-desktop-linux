const test = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");

test("healthcheck emits machine-readable healthy output for the installed bridge", () => {
  const output = execFileSync(process.execPath, ["scripts/chrome-bridge-healthcheck.js", "--json"], { encoding: "utf8" });
  const report = JSON.parse(output);
  assert.equal(report.ok, true);
  assert.ok(report.checks.some((item) => item.name === "native-host-manifest" && item.ok));
  assert.ok(report.checks.some((item) => item.name === "extension-installed" && item.ok));
});

test("--no-live disables the runner without weakening static checks", () => {
  const output = execFileSync(process.execPath, ["scripts/chrome-bridge-healthcheck.js", "--json", "--no-live"], { encoding: "utf8" });
  const report = JSON.parse(output);
  assert.equal(report.ok, true);
  assert.equal(report.liveBridge.status, "disabled");
});

test("strict mode requires a successful live round-trip", () => {
  const result = require("node:child_process").spawnSync(process.execPath, ["scripts/chrome-bridge-healthcheck.js", "--json", "--strict", "--no-live"], { encoding: "utf8" });
  assert.notEqual(result.status, 0);
  assert.equal(JSON.parse(result.stdout).liveBridge.status, "disabled");
});
