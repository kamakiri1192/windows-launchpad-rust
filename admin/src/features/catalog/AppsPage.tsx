import { useState } from "react";
import { Badge, Button, Select, TextField } from "@radix-ui/themes";
import {
  Plus,
  Search,
  ArrowUpRight,
  ChevronLeft,
  ChevronRight,
  Layers3,
  FolderOpen,
  Monitor,
  Apple,
} from "lucide-react";
import { useCatalog } from "./CatalogContext";
import { AppEditor } from "./AppEditor";
import { type CatalogApp, platforms, ruleLabels } from "./schema";
import { AppIcon, OS, PageHeading } from "../../shared/ui";
export default function AppsPage() {
  const { document } = useCatalog();
  const [search, setSearch] = useState("");
  const [os, setOs] = useState("all");
  const [folder, setFolder] = useState("all");
  const [page, setPage] = useState(0);
  const [editing, setEditing] = useState<CatalogApp | null>(null);
  const filtered = document.apps.filter(
    (a) =>
      (folder === "all" || a.folder_id === folder) &&
      (os === "all" ||
        platforms(a, document.folders).includes(os as "macos" | "windows")) &&
      `${a.name} ${a.id} ${JSON.stringify(a.rules)}`
        .toLocaleLowerCase()
        .includes(search.toLocaleLowerCase()),
  );
  const maxPage = Math.max(0, Math.ceil(filtered.length / 20) - 1);
  const current = Math.min(page, maxPage);
  return (
    <>
      <PageHeading
        eyebrow="CATALOG / APPLICATIONS"
        title="アプリカタログ"
        description="アプリと分類ルールを、一か所で管理。"
        action={
          <Button
            size="3"
            onClick={() =>
              setEditing({
                id: `app-${crypto.randomUUID().slice(0, 8)}`,
                name: "",
                folder_id: document.folders[0].id,
                enabled: true,
                rules: [{ type: "mac_bundle", bundle: "", product: "" }],
                icon: null,
              })
            }
          >
            <Plus size={16} />
            アプリを追加
          </Button>
        }
      />
      <div className="stats-grid">
        {[
          {
            label: "登録アプリ",
            value: document.apps.length,
            icon: Layers3,
            note: "管理中のアプリ・製品ルール",
          },
          {
            label: "分類フォルダ",
            value: document.folders.length,
            icon: FolderOpen,
            note: "日本語と英語の表示名に対応",
          },
          {
            label: "Windows",
            value: document.apps.filter((a) =>
              platforms(a, document.folders).includes("windows"),
            ).length,
            icon: Monitor,
            note: "Windows対象フォルダの登録数",
          },
          {
            label: "Mac",
            value: document.apps.filter((a) =>
              platforms(a, document.folders).includes("macos"),
            ).length,
            icon: Apple,
            note: "Mac対象フォルダの登録数",
          },
        ].map((s) => (
          <div className="stat-card" key={s.label}>
            <span>
              {s.label}
              <s.icon size={17} />
            </span>
            <strong>
              {s.value}
              <small>件</small>
            </strong>
            <p>{s.note}</p>
          </div>
        ))}
      </div>
      <div className="catalog-toolbar">
        <div className="toolbar-title">
          <h2>すべてのアプリ</h2>
          <Badge color="gray">{filtered.length}</Badge>
        </div>
        <div className="filter-row">
          <TextField.Root
            placeholder="アプリ名・識別IDで検索…"
            aria-label="アプリを検索"
            value={search}
            onChange={(e) => {
              setSearch(e.target.value);
              setPage(0);
            }}
          >
            <TextField.Slot>
              <Search size={16} />
            </TextField.Slot>
          </TextField.Root>
          <Select.Root
            value={os}
            onValueChange={(v) => {
              setOs(v);
              setPage(0);
            }}
          >
            <Select.Trigger aria-label="OSで絞り込み" />
            <Select.Content>
              <Select.Item value="all">すべてのOS</Select.Item>
              <Select.Item value="windows">Windows</Select.Item>
              <Select.Item value="macos">Mac</Select.Item>
            </Select.Content>
          </Select.Root>
          <Select.Root
            value={folder}
            onValueChange={(v) => {
              setFolder(v);
              setPage(0);
            }}
          >
            <Select.Trigger aria-label="フォルダで絞り込み" />
            <Select.Content>
              <Select.Item value="all">すべてのフォルダ</Select.Item>
              {document.folders.map((f) => (
                <Select.Item key={f.id} value={f.id}>
                  {f.names.ja}
                </Select.Item>
              ))}
            </Select.Content>
          </Select.Root>
        </div>
      </div>
      <div className="table-shell">
        <table>
          <thead>
            <tr>
              <th>アプリ</th>
              <th>分類先フォルダ</th>
              <th>対象OS</th>
              <th>識別ルール</th>
              <th>状態</th>
              <th>
                <span className="sr-only">編集</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {filtered.slice(current * 20, current * 20 + 20).map((a) => (
              <tr key={a.id}>
                <td>
                  <button className="app-cell" onClick={() => setEditing(a)}>
                    <AppIcon app={a} />
                    <span>
                      <strong>{a.name}</strong>
                      <small>{a.id}</small>
                    </span>
                  </button>
                </td>
                <td>
                  <span className="folder-cell">
                    <FolderOpen size={15} />
                    {
                      document.folders.find((f) => f.id === a.folder_id)?.names
                        .ja
                    }
                  </span>
                </td>
                <td>
                  <OS values={platforms(a, document.folders)} />
                </td>
                <td>
                  <span className="rule-summary">
                    {ruleLabels[a.rules[0].type]}
                    {a.rules.length > 1 && (
                      <small>ほか {a.rules.length - 1} 件</small>
                    )}
                  </span>
                </td>
                <td>
                  <span
                    className={`status-dot ${a.enabled ? "enabled" : ""}`}
                  />
                  {a.enabled ? "有効" : "停止中"}
                </td>
                <td>
                  <button
                    className="icon-button"
                    aria-label={`${a.name}を編集`}
                    onClick={() => setEditing(a)}
                  >
                    <ArrowUpRight size={17} />
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {filtered.length === 0 && (
          <div className="empty-state">
            <Search size={28} />
            <h3>条件に一致するアプリはありません</h3>
            <p>検索語やフィルターを変更してください。</p>
          </div>
        )}
        <div className="table-footer">
          <small>
            {filtered.length}件中 {filtered.length ? current * 20 + 1 : 0}–
            {Math.min(current * 20 + 20, filtered.length)}件を表示
          </small>
          <div>
            <Button
              variant="ghost"
              color="gray"
              disabled={current === 0}
              aria-label="前のページ"
              onClick={() => setPage(current - 1)}
            >
              <ChevronLeft size={16} />
            </Button>
            <span>
              {current + 1} / {maxPage + 1}
            </span>
            <Button
              variant="ghost"
              color="gray"
              disabled={current >= maxPage}
              aria-label="次のページ"
              onClick={() => setPage(current + 1)}
            >
              <ChevronRight size={16} />
            </Button>
          </div>
        </div>
      </div>
      <p className="footnote">
        分類データは初回配置と設定リセット時に参照されます。保存済みのユーザー配置は維持されます。
      </p>
      {editing && (
        <AppEditor initial={editing} onClose={() => setEditing(null)} />
      )}
    </>
  );
}
