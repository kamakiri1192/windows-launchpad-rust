import { defineConfig } from "@playwright/test";
import { execFileSync } from "node:child_process";
// Every run owns fresh private state and public artifacts, including its password/token files.
const runtime = JSON.parse(
  execFileSync("php", ["scripts/test-config.php"], { encoding: "utf8" }),
);
export default defineConfig({
  testDir: "tests/e2e",
  fullyParallel: false,
  workers: 1,
  metadata: { testRoot: runtime.root },
  globalTeardown: "./tests/global-teardown.ts",
  use: {
    baseURL: "http://127.0.0.1:5173",
    headless: true,
    channel: process.env.CI ? undefined : "chrome",
    screenshot: "only-on-failure",
  },
  reporter: "list",
  webServer: [
    {
      command: `php -S 127.0.0.1:8787 -t ${runtime.public}`,
      env: { LAUNCHPAD_ADMIN_CONFIG: runtime.config },
      port: 8787,
      reuseExistingServer: false,
    },
    { command: "npm run dev", port: 5173, reuseExistingServer: false },
  ],
});
