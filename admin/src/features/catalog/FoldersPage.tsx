import { useState } from "react";
import {
  Button,
  Dialog,
  TextField,
  Checkbox,
  TextArea,
} from "@radix-ui/themes";
import { FolderOpen, Plus, ArrowUpRight } from "lucide-react";
import { useCatalog } from "./CatalogContext";
import { folderSchema, type Folder } from "./schema";
import { OS, PageHeading, Notice } from "../../shared/ui";
export default function FoldersPage() {
  const { document, update } = useCatalog();
  const [editing, setEditing] = useState<Folder | null>(null);
  const [original, setOriginal] = useState("");
  const [error, setError] = useState("");
  function edit(f: Folder) {
    setOriginal(f.id);
    setEditing(structuredClone(f));
    setError("");
  }
  return (
    <>
      <PageHeading
        eyebrow="CATALOG / FOLDERS"
        title="分類フォルダ"
        description="用途と製品に合わせて、アプリの居場所をつくる。"
        action={
          <Button
            size="3"
            onClick={() =>
              edit({
                id: `folder-${crypto.randomUUID().slice(0, 8)}`,
                names: { ja: "", en: "" },
                platforms: ["windows", "macos"],
                evidence: "管理者が確認",
              })
            }
          >
            <Plus size={16} />
            フォルダを追加
          </Button>
        }
      />
      <div className="folder-grid">
        {document.folders.map((f) => (
          <button key={f.id} className="folder-card" onClick={() => edit(f)}>
            <div className="folder-card-top">
              <span className="folder-icon">
                <FolderOpen size={24} />
              </span>
              <ArrowUpRight size={17} />
            </div>
            <h3>{f.names.ja}</h3>
            <p>{f.names.en}</p>
            <div className="folder-card-bottom">
              <OS values={f.platforms} />
              <span>
                {document.apps.filter((a) => a.folder_id === f.id).length}{" "}
                アプリ
              </span>
            </div>
          </button>
        ))}
      </div>
      <Dialog.Root
        open={!!editing}
        onOpenChange={(v) => {
          if (!v) setEditing(null);
        }}
      >
        <Dialog.Content maxWidth="540px">
          <Dialog.Title>フォルダを編集</Dialog.Title>
          <Dialog.Description>表示名と対象OSを設定します。</Dialog.Description>
          {editing && (
            <form
              className="form-stack"
              onSubmit={(e) => {
                e.preventDefault();
                const parsed = folderSchema.safeParse(editing);
                if (!parsed.success) {
                  setError(parsed.error.issues[0].message);
                  return;
                }
                if (
                  document.folders.some(
                    (f) => f.id === editing.id && f.id !== original,
                  )
                ) {
                  setError("同じIDが存在します。");
                  return;
                }
                update({
                  ...document,
                  folders: document.folders.some((f) => f.id === original)
                    ? document.folders.map((f) =>
                        f.id === original ? editing : f,
                      )
                    : [...document.folders, editing],
                  apps: document.apps.map((a) =>
                    a.folder_id === original
                      ? { ...a, folder_id: editing.id }
                      : a,
                  ),
                });
                setEditing(null);
              }}
            >
              <Notice error={error} />
              <label>
                フォルダID
                <TextField.Root
                  required
                  pattern="[a-z0-9-]+"
                  value={editing.id}
                  onChange={(e) =>
                    setEditing({ ...editing, id: e.target.value })
                  }
                />
              </label>
              {Object.entries(editing.names).map(([locale, name]) => (
                <label key={locale}>
                  {locale === "ja"
                    ? "日本語名"
                    : locale === "en"
                      ? "英語名"
                      : `${locale} の表示名`}
                  <TextField.Root
                    required
                    value={name}
                    onChange={(e) =>
                      setEditing({
                        ...editing,
                        names: { ...editing.names, [locale]: e.target.value },
                      })
                    }
                  />
                </label>
              ))}
              <div className="inline-field">
                <span>対象OS</span>
                {(["windows", "macos"] as const).map((os) => (
                  <label key={os}>
                    <Checkbox
                      checked={editing.platforms.includes(os)}
                      onCheckedChange={(v) =>
                        setEditing({
                          ...editing,
                          platforms: v
                            ? [...editing.platforms, os]
                            : editing.platforms.filter((p) => p !== os),
                        })
                      }
                    />
                    {os === "macos" ? "Mac" : "Windows"}
                  </label>
                ))}
              </div>
              <label>
                分類の根拠・メモ
                <TextArea
                  required
                  value={editing.evidence}
                  onChange={(e) =>
                    setEditing({ ...editing, evidence: e.target.value })
                  }
                />
              </label>
              <div className="dialog-footer">
                <Button
                  type="button"
                  color="red"
                  variant="ghost"
                  disabled={
                    document.apps.some((a) => a.folder_id === original) ||
                    document.folders.length <= 1
                  }
                  onClick={() => {
                    update({
                      ...document,
                      folders: document.folders.filter(
                        (f) => f.id !== original,
                      ),
                    });
                    setEditing(null);
                  }}
                >
                  削除
                </Button>
                <span className="spacer" />
                <Button
                  type="button"
                  color="gray"
                  variant="soft"
                  onClick={() => setEditing(null)}
                >
                  キャンセル
                </Button>
                <Button type="submit">変更を反映</Button>
              </div>
              <small>
                登録アプリがあるフォルダは削除できません。先に分類先を変更してください。
              </small>
            </form>
          )}
        </Dialog.Content>
      </Dialog.Root>
    </>
  );
}
