import { useState } from "react";
import {
  Button,
  Dialog,
  TextField,
  Select,
  Switch,
  Badge,
} from "@radix-ui/themes";
import { Plus, Trash2, Upload, Search, ImagePlus, X } from "lucide-react";
import { api, iconUrl, message } from "../../shared/api";
import { Notice } from "../../shared/ui";
import { useCatalog } from "./CatalogContext";
import {
  appSchema,
  ruleFields,
  ruleLabels,
  type CatalogApp,
  type RuleKind,
  type Rule,
} from "./schema";
type Candidate = {
  name: string;
  bundle: string;
  publisher: string;
  artwork: string;
  store_url: string;
};
const fieldLabels: Record<string, string> = {
  publisher: "発行元",
  product_name: "製品名（OS情報）",
  bundle: "Bundle ID",
  product: "製品キー",
  prefix: "Bundle IDの接頭辞",
  directory: "インストール先フォルダ名",
  executable: "実行ファイル名（小文字）",
  aumid: "AUMID",
  id: "Steam ID（steam:12345）",
  group: "組み込み判定グループ",
};
export function AppEditor({
  initial,
  onClose,
}: {
  initial: CatalogApp;
  onClose: () => void;
}) {
  const { document, update } = useCatalog();
  const [app, setApp] = useState(initial);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [term, setTerm] = useState(app.name);
  const [country, setCountry] = useState("jp");
  const [candidates, setCandidates] = useState<Candidate[] | null>(null);
  function editRule(index: number, rule: Rule) {
    setApp({
      ...app,
      rules: app.rules.map((r, i) => (i === index ? rule : r)),
    });
  }
  async function upload(file: File) {
    setBusy(true);
    setError("");
    try {
      const data = new FormData();
      data.append("file", file);
      const icon = await api<CatalogApp["icon"]>("icon_upload", data);
      setApp((a) => ({ ...a, icon }));
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Dialog.Root
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <Dialog.Content maxWidth="820px" className="editor-dialog">
        <Dialog.Title>アプリの分類を編集</Dialog.Title>
        <Dialog.Description size="2">
          表示名ではなく、OSが持つ識別情報で分類します。
        </Dialog.Description>
        <Notice error={error} />
        <form
          onSubmit={(e) => {
            e.preventDefault();
            const parsed = appSchema.safeParse(app);
            if (!parsed.success) {
              setError(parsed.error.issues[0].message);
              return;
            }
            if (
              document.apps.some((a) => a.id === app.id && a.id !== initial.id)
            ) {
              setError("同じアプリIDが存在します。");
              return;
            }
            update({
              ...document,
              apps: document.apps.some((a) => a.id === initial.id)
                ? document.apps.map((a) => (a.id === initial.id ? app : a))
                : [...document.apps, app],
            });
            onClose();
          }}
        >
          <div className="editor-columns">
            <div className="form-stack">
              <label>
                アプリ名
                <TextField.Root
                  required
                  value={app.name}
                  onChange={(e) => setApp({ ...app, name: e.target.value })}
                />
              </label>
              <label>
                管理ID
                <TextField.Root
                  required
                  pattern="[a-z0-9-]+"
                  value={app.id}
                  onChange={(e) => setApp({ ...app, id: e.target.value })}
                />
              </label>
              <label>
                分類先フォルダ
                <Select.Root
                  value={app.folder_id}
                  onValueChange={(folder_id) => setApp({ ...app, folder_id })}
                >
                  <Select.Trigger />
                  <Select.Content>
                    {document.folders.map((f) => (
                      <Select.Item key={f.id} value={f.id}>
                        {f.names.ja} / {f.names.en}
                      </Select.Item>
                    ))}
                  </Select.Content>
                </Select.Root>
              </label>
              <label className="switch-row">
                <Switch
                  checked={app.enabled}
                  onCheckedChange={(enabled) => setApp({ ...app, enabled })}
                />
                分類ルールを有効にする
              </label>
            </div>
            <div className="icon-editor">
              <div className="icon-preview">
                {app.icon ? (
                  <img alt="アプリアイコン" src={iconUrl(app.icon.path)} />
                ) : (
                  <ImagePlus size={34} />
                )}
              </div>
              <label className="upload-button">
                <Upload size={15} />
                画像をアップロード
                <input
                  type="file"
                  accept="image/png,image/jpeg,image/webp"
                  disabled={busy}
                  onChange={(e) => {
                    const f = e.target.files?.[0];
                    if (f) void upload(f);
                  }}
                />
              </label>
              <small>PNG・JPEG・WebP / 最大2 MiB</small>
              {app.icon && (
                <Button
                  size="1"
                  variant="ghost"
                  color="gray"
                  type="button"
                  onClick={() => setApp({ ...app, icon: null })}
                >
                  アイコンを解除
                </Button>
              )}
            </div>
          </div>
          <section className="rule-section">
            <div className="section-heading">
              <h3>識別ルール</h3>
              <Button
                type="button"
                size="1"
                variant="soft"
                onClick={() =>
                  setApp({
                    ...app,
                    rules: [
                      ...app.rules,
                      { type: "mac_bundle", bundle: "", product: app.id },
                    ],
                  })
                }
              >
                <Plus size={14} />
                ルールを追加
              </Button>
            </div>
            {app.rules.map((rule, i) => (
              <div key={i} className="rule-card">
                <div className="rule-header">
                  <Badge color="gray">{String(i + 1).padStart(2, "0")}</Badge>
                  <Select.Root
                    value={rule.type}
                    onValueChange={(value) => {
                      const kind = value as RuleKind;
                      editRule(i, {
                        type: kind,
                        ...Object.fromEntries(
                          ruleFields[kind].map((f) => [
                            f,
                            f === "versioned_executable" ? false : "",
                          ]),
                        ),
                      });
                    }}
                  >
                    <Select.Trigger />
                    <Select.Content>
                      {Object.entries(ruleLabels).map(([value, label]) => (
                        <Select.Item key={value} value={value}>
                          {label}
                        </Select.Item>
                      ))}
                    </Select.Content>
                  </Select.Root>
                  <button
                    className="icon-button"
                    aria-label={`ルール${i + 1}を削除`}
                    type="button"
                    disabled={app.rules.length === 1}
                    onClick={() =>
                      setApp({
                        ...app,
                        rules: app.rules.filter((_, n) => n !== i),
                      })
                    }
                  >
                    <Trash2 size={15} />
                  </button>
                </div>
                <div className="rule-fields">
                  {ruleFields[rule.type].map((f) =>
                    f === "versioned_executable" ? (
                      <label className="switch-row" key={f}>
                        <Switch
                          checked={Boolean(rule[f])}
                          onCheckedChange={(value) =>
                            editRule(i, { ...rule, [f]: value })
                          }
                        />
                        実行ファイルのバージョン接尾辞を許可
                      </label>
                    ) : (
                      <label key={f}>
                        {fieldLabels[f]}
                        <TextField.Root
                          required
                          value={String(rule[f] ?? "")}
                          onChange={(e) =>
                            editRule(i, { ...rule, [f]: e.target.value })
                          }
                        />
                      </label>
                    ),
                  )}
                </div>
                {ruleFields[rule.type].length === 0 && (
                  <small>アプリ内の検証済み判定ロジックを使用します。</small>
                )}
              </div>
            ))}
          </section>
          <details className="icon-search">
            <summary>
              <Search size={15} /> App Storeからアイコン候補を探す
            </summary>
            <p>Mac App Storeの公開情報を検索し、確認して取り込みます。</p>
            <div className="search-controls">
              <TextField.Root
                aria-label="アイコン検索語"
                value={term}
                onChange={(e) => setTerm(e.target.value)}
              />
              <Select.Root value={country} onValueChange={setCountry}>
                <Select.Trigger />
                <Select.Content>
                  <Select.Item value="jp">日本</Select.Item>
                  <Select.Item value="us">米国</Select.Item>
                </Select.Content>
              </Select.Root>
              <Button
                type="button"
                disabled={busy}
                onClick={async () => {
                  setBusy(true);
                  setError("");
                  try {
                    setCandidates(
                      await api<Candidate[]>("icon_search", undefined, {
                        term,
                        country,
                      }),
                    );
                  } catch (e) {
                    setError(message(e));
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                検索
              </Button>
            </div>
            {candidates?.length === 0 && (
              <p>候補がありません。画像アップロードをご利用ください。</p>
            )}
            <div className="icon-results">
              {candidates?.map((c) => (
                <button
                  type="button"
                  key={c.artwork}
                  disabled={busy}
                  onClick={async () => {
                    setBusy(true);
                    setError("");
                    try {
                      const icon = await api<CatalogApp["icon"]>(
                        "icon_import",
                        { url: c.artwork },
                      );
                      setApp((a) => ({ ...a, icon }));
                    } catch (e) {
                      setError(message(e));
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  <img src={c.artwork} alt="" />
                  <span>
                    {c.name}
                    <small>{c.publisher}</small>
                    <small>{c.bundle}</small>
                  </span>
                  <Plus size={16} />
                </button>
              ))}
            </div>
          </details>
          <div className="dialog-footer">
            <Button
              type="button"
              color="gray"
              variant="soft"
              onClick={onClose}
              disabled={busy}
            >
              キャンセル
            </Button>
            <Button type="submit" disabled={busy}>
              変更を反映
            </Button>
          </div>
        </form>
        <Dialog.Close>
          <button
            type="button"
            aria-label="閉じる"
            className="dialog-close icon-button"
          >
            <X size={18} />
          </button>
        </Dialog.Close>
      </Dialog.Content>
    </Dialog.Root>
  );
}
