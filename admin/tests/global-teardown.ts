import type { FullConfig } from "@playwright/test";
import { rmSync } from "node:fs";
import { basename } from "node:path";
export default function teardown(config: FullConfig) {
  const root = config.metadata.testRoot;
  if (
    typeof root === "string" &&
    basename(root).startsWith("launchpad-admin-e2e-")
  )
    rmSync(root, { recursive: true, force: true });
}
