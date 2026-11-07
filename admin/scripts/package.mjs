import { cp, mkdir, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("../../", import.meta.url));
const output = `${root}target/admin-package`;
await rm(output, { recursive: true, force: true });
await mkdir(`${output}/server/data`, { recursive: true });
await cp(`${root}admin/dist`, `${output}/public_html`, { recursive: true });
await cp(`${root}admin/server`, `${output}/server`, {
  recursive: true,
  filter: (path) => !path.endsWith("/config.php"),
});
await cp(
  `${root}assets/catalog/catalog-v1.json`,
  `${output}/server/data/catalog-v1.json`,
);
console.log(`Package: ${output} (configure server/config.php before use)`);
