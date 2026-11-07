import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Button,
  Badge,
  Checkbox,
  Select,
  TextField,
  Switch,
} from "@radix-ui/themes";
import {
  Inbox,
  Upload,
  RefreshCw,
  CheckCheck,
  Download,
  Sparkles,
} from "lucide-react";
import { api, message, iconUrl } from "../../shared/api";
import { Notice, OS, PageHeading } from "../../shared/ui";
import { useCatalog } from "../catalog/CatalogContext";
import type { Draft, Rule, CatalogApp } from "../catalog/schema";
type Candidate = {
  id: string;
  name: string;
  platform: "windows" | "macos";
  rule: Rule | null;
  folder_id: string | null;
  confidence: "high" | "review";
  reason: string;
  icon: CatalogApp["icon"];
  existing_id: string | null;
  source: string;
};
type Settings = { terms: string[]; country: string; auto_accept: boolean };
type CollectionStatus = {
  settings: Settings;
  last_run: {
    at: string;
    added: number;
    accepted: number;
    errors: string[];
  } | null;
};
export default function CollectionPage() {
  const { document: doc, revision, dirty, accept } = useCatalog();
  const client = useQueryClient();
  const inbox = useQuery({
    queryKey: ["inbox"],
    queryFn: () => api<Candidate[]>("inbox"),
  });
  const status = useQuery({
    queryKey: ["collection"],
    queryFn: () => api<CollectionStatus>("collection"),
  });
  const [selected, setSelected] = useState<string[]>([]);
  const [folders, setFolders] = useState<Record<string, string>>({});
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [busy, setBusy] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [filter, setFilter] = useState("all");
  const items = inbox.data ?? [];
  const visible = items.filter(
    (i) => filter === "all" || i.platform === filter,
  );
  const ready = items.filter(
    (i) => i.confidence === "high" && i.rule && i.folder_id,
  );
  async function refresh() {
    await Promise.all([
      client.invalidateQueries({ queryKey: ["inbox"] }),
      client.invalidateQueries({ queryKey: ["collection"] }),
    ]);
  }
  async function run(fn: () => Promise<void>) {
    setBusy(true);
    setError("");
    setSuccess("");
    try {
      await fn();
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  }
  async function importFile(file: File) {
    await run(async () => {
      if (file.size > 32 * 1024 * 1024)
        throw new Error("インベントリは32 MiBまでです。");
      const data = JSON.parse(await file.text());
      if (data.schema_version !== 1 || !Array.isArray(data.apps))
        throw new Error("Launchpadの収集JSONを選んでください。");
      let count = 0;
      for (let i = 0; i < data.apps.length; i += 10) {
        const r = await api<{ added: number }>("ingest", {
          schema_version: 1,
          source: "desktop",
          apps: data.apps.slice(i, i + 10),
        });
        count += r.added;
      }
      await refresh();
      setSuccess(
        `${count}件の登録・更新候補を作成しました。既知のアプリは重複登録しません。`,
      );
    });
  }
  async function approve(ids: string[]) {
    await run(async () => {
      const result = await api<{ draft: Draft; accepted: number }>(
        "accept_inbox",
        {
          revision,
          selections: ids.map((id) => ({
            id,
            folder_id: folders[id] ?? items.find((i) => i.id === id)?.folder_id,
          })),
        },
      );
      accept(result.draft);
      setSelected([]);
      await refresh();
      setSuccess(
        `${result.accepted}件を下書きに取り込みました。公開画面から配信できます。`,
      );
    });
  }
  const config = settings ?? status.data?.settings;
  return (
    <>
      <PageHeading
        eyebrow="OPERATIONS / COLLECTION"
        title="収集・確認"
        description="登録は自動で。迷うものだけ、確認する。"
        action={
          <label className="import-button">
            <Upload size={16} />
            端末の収集JSONを取り込む
            <input
              type="file"
              accept="application/json,.json"
              disabled={busy}
              onChange={(e) => {
                const f = e.target.files?.[0];
                if (f) void importFile(f);
                e.target.value = "";
              }}
            />
          </label>
        }
      />
      <Notice
        error={
          error ||
          (inbox.error
            ? message(inbox.error)
            : status.error
              ? message(status.error)
              : "")
        }
        success={success}
      />
      <div className="collection-intro">
        <div>
          <span className="eyebrow">01 · COLLECT</span>
          <h3>端末・公開ストアから収集</h3>
          <p>アプリ名、OS識別情報、アイコンをまとめて取得。</p>
        </div>
        <div>
          <span className="eyebrow">02 · CLASSIFY</span>
          <h3>分類候補を自動で作成</h3>
          <p>既存ルールと公式カテゴリを照合。重複は除外。</p>
        </div>
        <div>
          <span className="eyebrow">03 · DELIVER</span>
          <h3>確認して、まとめて公開</h3>
          <p>判定済みは一括処理。判断できないものだけ確認。</p>
        </div>
      </div>
      <details className="collection-setup">
        <summary>
          <Download size={16} />
          Mac／Windowsから実際のアプリを収集する
        </summary>
        <p>
          管理者の端末で一度実行すると、インストール済みアプリとアイコンをまとめてJSONに書き出します。
        </p>
        <pre>launchpad-windows --export-catalog-inventory inventory.json</pre>
        <p>
          作成されたJSONをこの画面へ取り込んでください。ローカルのファイルパス・起動引数は含めず、利用者からの自動送信も行いません。Windowsでは
          .exe を付けて実行できます。
        </p>
      </details>
      <div className="catalog-toolbar">
        <div className="toolbar-title">
          <h2>登録・更新候補</h2>
          <Badge>{items.length}</Badge>
          <span className="spacer" />
          <Button
            variant="soft"
            disabled={busy || dirty || !ready.length}
            onClick={() => void approve(ready.map((i) => i.id))}
          >
            <CheckCheck size={15} />
            判定済み {ready.length}件を取り込む
          </Button>
        </div>
        <div className="filter-row">
          <Select.Root value={filter} onValueChange={setFilter}>
            <Select.Trigger />
            <Select.Content>
              <Select.Item value="all">すべてのOS</Select.Item>
              <Select.Item value="windows">Windows</Select.Item>
              <Select.Item value="macos">Mac</Select.Item>
            </Select.Content>
          </Select.Root>
          <span className="spacer" />
          <Button
            color="gray"
            variant="soft"
            disabled={!selected.length || busy}
            onClick={() =>
              void run(async () => {
                await api("dismiss_inbox", { ids: selected });
                setSelected([]);
                await refresh();
              })
            }
          >
            選択した候補を除外
          </Button>
          <Button
            disabled={!selected.length || busy || dirty}
            onClick={() => void approve(selected)}
          >
            選択した {selected.length}件を取り込む
          </Button>
        </div>
        {dirty && <p>先に編集中の下書きを保存してください。</p>}
      </div>
      <div className="table-shell">
        <table>
          <thead>
            <tr>
              <th>
                <Checkbox
                  aria-label="表示中の候補をすべて選択"
                  checked={
                    visible.length > 0 &&
                    visible
                      .filter((i) => i.rule)
                      .every((i) => selected.includes(i.id))
                  }
                  onCheckedChange={(v) =>
                    setSelected(
                      v ? visible.filter((i) => i.rule).map((i) => i.id) : [],
                    )
                  }
                />
              </th>
              <th>アプリ・収集元</th>
              <th>対象OS</th>
              <th>分類先</th>
              <th>判定根拠</th>
            </tr>
          </thead>
          <tbody>
            {visible.map((item) => (
              <tr key={item.id}>
                <td>
                  <Checkbox
                    aria-label={`${item.name}を選択`}
                    disabled={!item.rule}
                    checked={selected.includes(item.id)}
                    onCheckedChange={(v) =>
                      setSelected(
                        v
                          ? [...selected, item.id]
                          : selected.filter((id) => id !== item.id),
                      )
                    }
                  />
                </td>
                <td>
                  <div className="app-cell">
                    <span className="app-icon">
                      {item.icon ? (
                        <img src={iconUrl(item.icon.path)} alt="" />
                      ) : (
                        item.name.slice(0, 2)
                      )}
                    </span>
                    <span>
                      <strong>{item.name}</strong>
                      <small>
                        {item.source}
                        {item.existing_id ? " · アイコン補完" : ""}
                      </small>
                    </span>
                  </div>
                </td>
                <td>
                  <OS values={[item.platform]} />
                </td>
                <td>
                  <Select.Root
                    value={folders[item.id] ?? item.folder_id ?? "unassigned"}
                    onValueChange={(v) =>
                      setFolders({ ...folders, [item.id]: v })
                    }
                  >
                    <Select.Trigger aria-label={`${item.name}の分類先`} />
                    <Select.Content>
                      <Select.Item value="unassigned">分類先を選択</Select.Item>
                      {doc.folders
                        .filter((f) => f.platforms.includes(item.platform))
                        .map((f) => (
                          <Select.Item key={f.id} value={f.id}>
                            {f.names.ja}
                          </Select.Item>
                        ))}
                    </Select.Content>
                  </Select.Root>
                </td>
                <td>
                  <Badge
                    color={
                      item.rule && item.confidence === "high"
                        ? "green"
                        : "orange"
                    }
                  >
                    {!item.rule
                      ? "識別情報が不足"
                      : item.confidence === "high"
                        ? "判定済み"
                        : "要確認"}
                  </Badge>
                  <small className="candidate-reason">
                    {item.rule
                      ? item.reason
                      : "MCPで識別情報を調査・補完してください"}
                  </small>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {!items.length && (
          <div className="empty-state">
            <Inbox size={30} />
            <h3>確認待ちの候補はありません</h3>
            <p>端末の収集JSONを取り込むか、公開ストアから収集してください。</p>
          </div>
        )}
      </div>
      <section className="panel collection-panel">
        <div className="section-heading">
          <h2>
            <Sparkles size={19} />
            公開ストアから定期収集
          </h2>
          <Badge color="gray">Mac App Store</Badge>
        </div>
        <p>
          検索条件を保存すると、Cronから定期収集できます。公式カテゴリとBundle
          IDを持つ候補を作成し、アイコンも取得します。Windowsの正確な実行情報は端末の収集JSONから取り込みます。
        </p>
        {config && (
          <>
            <div className="form-stack">
              <label>
                検索テーマ（1行に1件・最大10件）
                <textarea
                  className="plain-textarea"
                  value={config.terms.join("\n")}
                  onChange={(e) =>
                    setSettings({
                      ...config,
                      terms: e.target.value.split("\n"),
                    })
                  }
                />
              </label>
              <div className="inline-field">
                <label>
                  ストア
                  <Select.Root
                    value={config.country}
                    onValueChange={(country) =>
                      setSettings({ ...config, country })
                    }
                  >
                    <Select.Trigger />
                    <Select.Content>
                      <Select.Item value="jp">日本</Select.Item>
                      <Select.Item value="us">米国</Select.Item>
                    </Select.Content>
                  </Select.Root>
                </label>
                <label className="switch-row">
                  <Switch
                    checked={config.auto_accept}
                    onCheckedChange={(auto_accept) =>
                      setSettings({ ...config, auto_accept })
                    }
                  />
                  判定済みの候補を自動で下書きへ保存
                </label>
              </div>
            </div>
            <div className="button-row">
              <Button
                variant="soft"
                disabled={busy || !settings}
                onClick={() =>
                  void run(async () => {
                    await api("configure_collection", settings);
                    setSettings(null);
                    await refresh();
                    setSuccess("収集条件を保存しました。");
                  })
                }
              >
                収集条件を保存
              </Button>
              <Button
                disabled={busy || dirty || !!settings}
                onClick={() =>
                  void run(async () => {
                    const result = {
                      added: 0,
                      accepted: 0,
                      errors: [] as string[],
                    };
                    let more = true;
                    while (more) {
                      const batch = await api<{
                        added: number;
                        accepted: number;
                        errors: string[];
                        has_more: boolean;
                      }>("collect", {});
                      result.added += batch.added;
                      result.accepted += batch.accepted;
                      result.errors.push(...batch.errors);
                      more = batch.has_more;
                    }
                    await refresh();
                    const draft = await api<Draft>("catalog");
                    accept(draft);
                    setSuccess(
                      `${result.added}件の候補を収集、${result.accepted}件を下書きへ保存しました。`,
                    );
                    if (result.errors.length)
                      setError(result.errors.join("\n"));
                  })
                }
              >
                <RefreshCw size={15} />
                {busy ? "処理中…" : "今すぐ収集"}
              </Button>
            </div>
            <small>
              公開は別操作です。自動収集だけで配信用JSONは変更されません。
            </small>
            {status.data?.last_run && (
              <p>
                前回実行:{" "}
                {new Date(status.data.last_run.at).toLocaleString("ja-JP")} ·{" "}
                {status.data.last_run.added}件の候補
                {status.data.last_run.errors.length > 0 &&
                  ` · ${status.data.last_run.errors.length}件の取得エラー`}
              </p>
            )}
          </>
        )}
      </section>
    </>
  );
}
