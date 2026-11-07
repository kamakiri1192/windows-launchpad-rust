import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Badge, Button, AlertDialog } from "@radix-ui/themes";
import {
  CloudUpload,
  FileJson,
  History,
  Download,
  CheckCircle2,
  ArrowRight,
} from "lucide-react";
import { useCatalog } from "../catalog/CatalogContext";
import type { Draft, Publication } from "../catalog/schema";
import { api, message } from "../../shared/api";
import { Notice, PageHeading } from "../../shared/ui";
export default function PublicationPage() {
  const { document: doc, revision, dirty, accept } = useCatalog();
  const client = useQueryClient();
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState<number | "publish" | null>(null);
  const status = useQuery({
    queryKey: ["publication"],
    queryFn: () => api<Publication>("status"),
  });
  async function publish() {
    if (!confirm) return;
    setBusy(true);
    setError("");
    try {
      const result = await api<{ draft: Draft; status: Publication }>(
        confirm === "publish" ? "publish" : "restore",
        { revision, ...(confirm === "publish" ? {} : { version: confirm }) },
      );
      accept(result.draft);
      client.setQueryData(["publication"], result.status);
      setSuccess(`v${result.draft.document.version} を公開しました。`);
      setConfirm(null);
    } catch (e) {
      setError(message(e));
      setConfirm(null);
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <PageHeading
        eyebrow="DELIVERY / RELEASES"
        title="公開と履歴"
        description="確認した変更だけを、世界中のLaunchpadへ。"
      />
      <Notice
        error={error || (status.error ? message(status.error) : "")}
        success={success}
      />
      <div className="release-overview">
        <section className="release-card">
          <div className="release-card-label">
            <FileJson size={20} />
            現在の下書き
            <Badge color={dirty ? "orange" : "gray"}>
              {dirty ? "未保存の変更あり" : "保存済み"}
            </Badge>
          </div>
          <h2>
            {doc.apps.filter((a) => a.enabled).length}{" "}
            <small>有効なアプリルール</small>
          </h2>
          <p>
            {doc.folders.length} フォルダ ·{" "}
            {doc.apps.reduce((n, a) => n + a.rules.length, 0)} 識別条件
          </p>
          <Button
            variant="soft"
            color="gray"
            onClick={() => {
              const url = URL.createObjectURL(
                new Blob([JSON.stringify(doc, null, 2)], {
                  type: "application/json",
                }),
              );
              const a = document.createElement("a");
              a.href = url;
              a.download = "catalog-draft.json";
              a.click();
              URL.revokeObjectURL(url);
            }}
          >
            <Download size={15} />
            JSONを書き出す
          </Button>
        </section>
        <div className="release-arrow">
          <ArrowRight size={24} />
        </div>
        <section className="release-card live">
          <div className="release-card-label">
            <CloudUpload size={20} />
            配信中のカタログ
            <Badge color={status.data?.published ? "green" : "gray"}>
              {status.data?.published ? "公開中" : "未公開"}
            </Badge>
          </div>
          <h2>
            {status.data?.published
              ? `v${status.data.published.version}`
              : "まだ公開されていません"}
          </h2>
          <p>
            {status.data?.published
              ? new Date(status.data.published.published_at).toLocaleString(
                  "ja-JP",
                )
              : "初回公開後に、配信用JSONが生成されます。"}
          </p>
          <Button
            disabled={dirty || busy || !status.data}
            onClick={() => setConfirm("publish")}
          >
            <CloudUpload size={15} />
            下書きを公開
          </Button>
        </section>
      </div>
      {status.data?.changes && (
        <section className="panel release-diff">
          <h2>公開前の変更確認</h2>
          <p>保存済みの下書きと、現在配信しているデータの差分です。</p>
          <div className="button-row">
            <Badge color="green">追加 {status.data.changes.added.length}</Badge>
            <Badge color="blue">
              変更 {status.data.changes.changed.length}
            </Badge>
            <Badge color="red">削除 {status.data.changes.removed.length}</Badge>
            {status.data.changes.folders_changed && (
              <Badge color="orange">フォルダ定義に変更あり</Badge>
            )}
          </div>
          {(
            [
              ["added", "追加"],
              ["changed", "変更"],
              ["removed", "削除"],
            ] as const
          ).map(
            ([key, label]) =>
              status.data!.changes[key].length > 0 && (
                <details key={key}>
                  <summary>{label}したアプリを確認</summary>
                  <ul>
                    {status.data!.changes[key].map((a) => (
                      <li key={a.id}>
                        {a.name}
                        <small>{a.id}</small>
                      </li>
                    ))}
                  </ul>
                </details>
              ),
          )}
        </section>
      )}
      <section className="delivery-note">
        <CheckCircle2 size={20} />
        <div>
          <h3>軽く、安定した配信</h3>
          <p>
            アプリは最大1日1回、静的JSONを取得。通信できない場合も、前回のデータで動作します。
          </p>
          <code>
            {new URL("catalog/catalog-v1.json", document.baseURI).href}
          </code>
          <p>
            更新後のデータは初回配置または設定リセット時に利用されます。利用者が整理した配置を自動で並べ替えることはありません。
          </p>
        </div>
      </section>
      <div className="section-heading">
        <h2>
          <History size={18} />
          公開履歴
        </h2>
        <span>差し戻しは新しいバージョンとして公開されます</span>
      </div>
      <div className="table-shell">
        <table>
          <thead>
            <tr>
              <th>バージョン</th>
              <th>公開日時</th>
              <th>アプリ</th>
              <th>フォルダ</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {status.data?.releases.map((r) => (
              <tr key={r.version}>
                <td>
                  <strong>v{r.version}</strong>{" "}
                  {status.data?.published?.version === r.version && (
                    <Badge color="green">配信中</Badge>
                  )}
                </td>
                <td>{new Date(r.published_at).toLocaleString("ja-JP")}</td>
                <td>{r.apps}</td>
                <td>{r.folders}</td>
                <td>
                  <Button
                    variant="ghost"
                    color="gray"
                    disabled={
                      dirty ||
                      busy ||
                      status.data?.published?.version === r.version
                    }
                    onClick={() => setConfirm(r.version)}
                  >
                    この版へ差し戻す
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {status.data?.releases.length === 0 && (
          <div className="empty-state">
            <History size={28} />
            <h3>公開履歴はまだありません</h3>
            <p>最初の公開を行うと、ここに記録されます。</p>
          </div>
        )}
      </div>
      <AlertDialog.Root
        open={confirm !== null}
        onOpenChange={(v) => {
          if (!v && !busy) setConfirm(null);
        }}
      >
        <AlertDialog.Content maxWidth="450px">
          <AlertDialog.Title>
            {confirm === "publish"
              ? "この下書きを公開しますか？"
              : `v${confirm} の内容に差し戻しますか？`}
          </AlertDialog.Title>
          <AlertDialog.Description>
            配信用JSONが更新されます。利用者のアプリは次回の取得時にこの版を受け取ります。
          </AlertDialog.Description>
          <div className="dialog-footer">
            <Button
              variant="soft"
              color="gray"
              disabled={busy}
              onClick={() => setConfirm(null)}
            >
              キャンセル
            </Button>
            <Button disabled={busy} onClick={() => void publish()}>
              {busy ? "公開中…" : "公開する"}
            </Button>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Root>
    </>
  );
}
