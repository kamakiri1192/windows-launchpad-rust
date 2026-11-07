import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button, AlertDialog, Badge } from "@radix-ui/themes";
import { Cable, Copy, KeyRound, ShieldCheck } from "lucide-react";
import { api, message } from "../../shared/api";
import { Notice, PageHeading } from "../../shared/ui";
type TokenStatus = {
  active: boolean;
  expires_at: number | null;
  suffix: string | null;
  token?: string;
};
export default function IntegrationsPage() {
  const client = useQueryClient();
  const status = useQuery({
    queryKey: ["mcp"],
    queryFn: () => api<TokenStatus>("mcp_status"),
  });
  const [token, setToken] = useState("");
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [action, setAction] = useState<"rotate" | "revoke" | null>(null);
  const [busy, setBusy] = useState(false);
  const endpoint = new URL("api/mcp.php", document.baseURI).href;
  const config = `[mcp_servers.launchpad]\nurl = "${endpoint}"\nbearer_token_env_var = "LAUNCHPAD_MCP_TOKEN"`;
  return (
    <>
      <PageHeading
        eyebrow="CONNECTIONS / MCP"
        title="MCP接続"
        description="Codexから、カタログの運用をそのまま。"
      />
      <Notice
        error={error || (status.error ? message(status.error) : "")}
        success={success}
      />
      <div className="connection-grid">
        <section className="panel">
          <div className="panel-icon">
            <Cable size={26} />
          </div>
          <h2>Launchpad MCPサーバー</h2>
          <p>
            Windows・Macの登録内容を読み取り、分類ルールの一括編集と公開を行えます。
          </p>
          <label>
            エンドポイント<code className="code-block">{endpoint}</code>
          </label>
          <div className="section-heading">
            <h3>Codexの接続設定</h3>
            <Button
              variant="ghost"
              onClick={async () => {
                try {
                  await navigator.clipboard.writeText(config);
                  setSuccess("設定をコピーしました。");
                } catch {
                  setError(
                    "コピーできませんでした。設定を選択してコピーしてください。",
                  );
                }
              }}
            >
              <Copy size={14} />
              コピー
            </Button>
          </div>
          <pre>{config}</pre>
          <small>トークンは環境変数 LAUNCHPAD_MCP_TOKEN に設定します。</small>
        </section>
        <section className="panel">
          <div className="section-heading">
            <h2>
              <KeyRound size={19} />
              アクセストークン
            </h2>
            <Badge color={status.data?.active ? "green" : "gray"}>
              {status.data?.active ? "有効" : "未発行・失効"}
            </Badge>
          </div>
          <p>
            管理者専用のトークンです。有効期限は90日。再発行すると以前のトークンは失効します。
          </p>
          {status.data?.active && (
            <p>
              末尾 ···{status.data.suffix}
              <br />
              期限{" "}
              {new Date(
                (status.data.expires_at ?? 0) * 1000,
              ).toLocaleDateString("ja-JP")}
            </p>
          )}
          {token && (
            <div className="token-result">
              <strong>この画面で一度だけ表示します</strong>
              <code>{token}</code>
              <Button
                variant="soft"
                onClick={async () => {
                  try {
                    await navigator.clipboard.writeText(token);
                    setSuccess("トークンをコピーしました。");
                  } catch {
                    setError("トークンを選択してコピーしてください。");
                  }
                }}
              >
                <Copy size={14} />
                トークンをコピー
              </Button>
            </div>
          )}
          <div className="button-row">
            <Button disabled={busy} onClick={() => setAction("rotate")}>
              {status.data?.active ? "再発行" : "トークンを発行"}
            </Button>
            <Button
              variant="soft"
              color="red"
              disabled={!status.data?.active || busy}
              onClick={() => setAction("revoke")}
            >
              失効
            </Button>
          </div>
          <div className="small-note">
            <ShieldCheck size={18} />
            <span>
              ログインと別の認証です。トークンそのものはサーバーに保存されません。
            </span>
          </div>
        </section>
      </div>
      <section className="panel">
        <h2>利用できる操作</h2>
        <div className="tool-list">
          {[
            ["list_inbox", "自動収集された未確認候補を取得"],
            ["import_inventory", "端末情報を一括取り込み"],
            ["accept_inbox", "候補をまとめて下書きへ保存"],
            ["collect_sources", "公開ストアから候補を収集"],
            ["get_catalog", "下書きと識別ルールを取得"],
            ["save_catalog", "一括編集した下書きを保存"],
            ["list_releases", "公開中の版と履歴を確認"],
            ["publish_catalog", "保存済み下書きを公開"],
            ["restore_release", "過去の版へ差し戻し"],
          ].map(([name, desc]) => (
            <div key={name}>
              <code>{name}</code>
              <span>{desc}</span>
            </div>
          ))}
        </div>
      </section>
      <AlertDialog.Root
        open={!!action}
        onOpenChange={(v) => {
          if (!v && !busy) setAction(null);
        }}
      >
        <AlertDialog.Content maxWidth="450px">
          <AlertDialog.Title>
            {action === "revoke"
              ? "接続トークンを失効しますか？"
              : "新しい接続トークンを発行しますか？"}
          </AlertDialog.Title>
          <AlertDialog.Description>
            現在のトークンを使うMCPクライアントは接続できなくなります。
          </AlertDialog.Description>
          <div className="dialog-footer">
            <Button
              variant="soft"
              color="gray"
              disabled={busy}
              onClick={() => setAction(null)}
            >
              キャンセル
            </Button>
            <Button
              disabled={busy}
              onClick={async () => {
                setBusy(true);
                setError("");
                try {
                  const result = await api<TokenStatus>(
                    action === "rotate" ? "mcp_rotate" : "mcp_revoke",
                    {},
                  );
                  setToken(result.token ?? "");
                  await client.invalidateQueries({ queryKey: ["mcp"] });
                  setAction(null);
                  setSuccess(
                    action === "rotate"
                      ? "接続トークンを発行しました。"
                      : "接続トークンを失効しました。",
                  );
                } catch (e) {
                  setError(message(e));
                } finally {
                  setBusy(false);
                }
              }}
            >
              実行する
            </Button>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Root>
    </>
  );
}
