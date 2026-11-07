import { test, expect } from "@playwright/test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
test("owner workflow: login, bulk ingestion, review, publish, MCP, revoke, mobile", async ({
  page,
  request,
}) => {
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "管理コンソールにログイン" }),
  ).toBeVisible();
  await page
    .getByLabel("パスワード", { exact: true })
    .fill("launchpad-local-test-password");
  await page.getByRole("button", { name: "ログイン", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "アプリカタログ", exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: "../target/qa-catalog-apps.png",
    fullPage: false,
  });
  await page.getByRole("link", { name: "収集・確認", exact: true }).click();
  const inventory = {
    schema_version: 1,
    source: "desktop",
    apps: [
      {
        name: "QA Mac Editor",
        platform: "macos",
        rule: {
          type: "mac_bundle",
          bundle: "org.launchpad.qa.editor",
          product: "qa-editor",
        },
        category: "public.app-category.developer-tools",
      },
      {
        name: "QA Windows Editor",
        platform: "windows",
        rule: {
          type: "windows_metadata",
          executable: "qa-editor.exe",
          publisher: "QA Vendor",
          product_name: "QA Editor",
          product: "qa-editor",
        },
      },
    ],
  };
  await page.locator(".import-button input").setInputFiles({
    name: "inventory.json",
    mimeType: "application/json",
    buffer: Buffer.from(JSON.stringify(inventory)),
  });
  await expect(
    page.getByText("2件の登録・更新候補を作成しました。", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "判定済み 1件を取り込む" }),
  ).toBeEnabled();
  await page.screenshot({
    path: "../target/qa-catalog-collection.png",
    fullPage: true,
  });
  await page.getByRole("button", { name: "判定済み 1件を取り込む" }).click();
  await expect(
    page.getByText("1件を下書きに取り込みました。", { exact: false }),
  ).toBeVisible();
  await page
    .getByRole("combobox", { name: "QA Windows Editorの分類先" })
    .click();
  await page
    .getByRole("option", { name: "Windows Dev tools", exact: true })
    .click();
  await page.getByRole("checkbox", { name: "QA Windows Editorを選択" }).check();
  await page.getByRole("button", { name: "選択した 1件を取り込む" }).click();
  await expect(
    page.getByRole("heading", { name: "確認待ちの候補はありません" }),
  ).toBeVisible();
  await page.getByRole("link", { name: "アプリカタログ", exact: true }).click();
  await page
    .getByRole("textbox", { name: "アプリを検索" })
    .fill("QA Mac Editor");
  await page.getByRole("button", { name: "QA Mac Editorを編集" }).click();
  await page
    .getByLabel("アプリ名", { exact: true })
    .fill("QA Mac Editor Updated");
  await page.getByRole("button", { name: "変更を反映" }).click();
  await page.getByRole("button", { name: "下書きを保存" }).click();
  await expect(
    page.getByText("下書きを保存しました。公開画面から配信できます。"),
  ).toBeVisible();
  await page.getByRole("link", { name: "公開と履歴" }).click();
  await expect(
    page.getByRole("heading", { name: "公開前の変更確認" }),
  ).toBeVisible();
  await page.screenshot({
    path: "../target/qa-catalog-publication.png",
    fullPage: true,
  });
  await page.getByRole("button", { name: "下書きを公開" }).click();
  await page.getByRole("button", { name: "公開する", exact: true }).click();
  await expect(page.getByText(/v\d+ を公開しました。/)).toBeVisible();
  const live = await request.get(
    "http://127.0.0.1:8787/catalog/catalog-v1.json",
  );
  expect(live.ok()).toBeTruthy();
  expect(
    (await live.json()).apps.some(
      (a: any) => a.name === "QA Mac Editor Updated",
    ),
  ).toBe(true);
  await page.getByRole("link", { name: "MCP接続" }).click();
  await page.getByRole("button", { name: /^(トークンを発行|再発行)$/ }).click();
  await page.getByRole("button", { name: "実行する" }).click();
  const token = await page.locator(".token-result code").innerText();
  const endpoint = new URL("http://127.0.0.1:8787/api/mcp.php");
  const client = new Client({
    name: "launchpad-integration-test",
    version: "1.0.0",
  });
  await client.connect(
    new StreamableHTTPClientTransport(endpoint, {
      requestInit: { headers: { Authorization: `Bearer ${token}` } },
    }),
  );
  const tools = await client.listTools();
  expect(tools.tools.map((t) => t.name)).toContain("import_inventory");
  const draft = await client.callTool({ name: "get_catalog", arguments: {} });
  expect(draft.isError).toBe(false);
  const parsed = JSON.parse((draft.content as any)[0].text);
  parsed.document.apps[0].name = "MCP integration edit";
  const saved = await client.callTool({
    name: "save_catalog",
    arguments: { document: parsed.document, revision: parsed.revision },
  });
  expect(saved.isError).toBe(false);
  const conflict = await client.callTool({
    name: "save_catalog",
    arguments: { document: parsed.document, revision: parsed.revision },
  });
  expect(conflict.isError).toBe(true);
  await client.close();
  await page.getByRole("button", { name: "失効", exact: true }).click();
  await page.getByRole("button", { name: "実行する" }).click();
  await expect(
    page.getByText("接続トークンを失効しました。", { exact: true }),
  ).toBeVisible();
  const revoked = await request.post(endpoint.href, {
    headers: {
      Authorization: `Bearer ${token}`,
      Accept: "application/json, text/event-stream",
    },
    data: { jsonrpc: "2.0", id: 1, method: "tools/list" },
  });
  expect(revoked.status()).toBe(401);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.reload();
  await page.getByRole("button", { name: "メニュー", exact: true }).click();
  await page.getByRole("link", { name: "収集・確認", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "収集・確認", exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: "../target/qa-catalog-mobile.png",
    fullPage: true,
  });
  expect(pageErrors).toEqual([]);
});
test("unauthenticated API and CSRF writes are rejected", async ({
  request,
}) => {
  expect(
    (
      await request.get("http://127.0.0.1:8787/api/index.php?action=catalog")
    ).status(),
  ).toBe(401);
  expect(
    (
      await request.post("http://127.0.0.1:8787/api/index.php?action=publish", {
        data: { revision: "bad" },
      })
    ).status(),
  ).toBe(403);
});
