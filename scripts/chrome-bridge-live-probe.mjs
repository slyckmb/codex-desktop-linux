#!/usr/bin/env node

// Read-only live probe. This is intended to run where browser-client can reach
// the Desktop/agent browser runtime; it never navigates, types, or mutates tabs.
try {
  const root = process.env.CODEX_CHROME_PLUGIN_ROOT || "/opt/codex-desktop/resources/plugins/openai-bundled/plugins/chrome";
  const { setupBrowserRuntime } = await import(`${root}/scripts/browser-client.mjs`);
  const agent = await setupBrowserRuntime();
  const browser = await agent.browsers.get("chrome");
  const tabs = await browser.user.openTabs();
  process.stdout.write(JSON.stringify({ status: "passed", tabCount: tabs.length }));
} catch (error) {
  const detail = error instanceof Error ? error.message : String(error);
  const unavailable = /runtime|transport|connect|available|extension|native|browser/i.test(detail);
  process.stdout.write(JSON.stringify({ status: unavailable ? "unavailable" : "failed", detail }));
  process.exitCode = unavailable ? 0 : 1;
}
