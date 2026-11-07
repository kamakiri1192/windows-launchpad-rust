import { useEffect, useState, type ReactNode } from "react";
import { Button, TextField, Callout } from "@radix-ui/themes";
import { ArrowRight, Layers3, LockKeyhole } from "lucide-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, setCsrf, message } from "../../shared/api";
type Session = { authenticated: boolean; csrf: string };
export function Auth({ children }: { children: ReactNode }) {
  const client = useQueryClient();
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const session = useQuery({
    queryKey: ["session"],
    queryFn: () => api<Session>("session"),
    retry: false,
  });
  useEffect(() => {
    if (session.data) setCsrf(session.data.csrf);
  }, [session.data]);
  useEffect(() => {
    const expire = () => {
      client.clear();
      void session.refetch();
    };
    window.addEventListener("session-expired", expire);
    return () => window.removeEventListener("session-expired", expire);
  }, [client]);
  if (session.data?.authenticated) return children;
  return (
    <div className="login-page">
      <div className="login-story">
        <div className="brand">
          <Layers3 size={24} /> Liquid Start <span>Console</span>
        </div>
        <div>
          <span className="eyebrow">A PLACE FOR EVERY APP</span>
          <h1>
            整理の仕組みを、
            <br />
            育てていこう。
          </h1>
          <p>
            WindowsとMacのアプリを、
            <br />
            世界中のデスクトップで、あるべき場所へ。
          </p>
          <div className="login-tiles">
            {["W", "M", "S", "A", "F", "D"].map((v, i) => (
              <span
                key={v}
                style={{ transform: `translateY(${i % 2 ? 24 : 0}px)` }}
              >
                {v}
              </span>
            ))}
          </div>
        </div>
        <small>Liquid Start · Application management</small>
      </div>
      <main className="login-form">
        <div className="lock-mark">
          <LockKeyhole size={22} />
        </div>
        <h2>管理コンソールにログイン</h2>
        <p>配信元の管理者用アカウントで続けます。</p>
        {(error || session.error) && (
          <Callout.Root color="red">
            <Callout.Text>{error || message(session.error)}</Callout.Text>
          </Callout.Root>
        )}
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            setBusy(true);
            setError("");
            try {
              const s = await api<Session>("login", { password });
              setCsrf(s.csrf);
              client.setQueryData(["session"], s);
              setPassword("");
            } catch (e) {
              setError(message(e));
            } finally {
              setBusy(false);
            }
          }}
        >
          <label>
            パスワード
            <TextField.Root
              size="3"
              type="password"
              autoComplete="current-password"
              required
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="管理者パスワード"
            />
          </label>
          <Button size="3" type="submit" disabled={!session.data || busy}>
            ログイン <ArrowRight size={16} />
          </Button>
        </form>
        <small>管理者専用 · 安全なセッション接続</small>
      </main>
    </div>
  );
}
export async function logout() {
  await api("logout", {});
  window.location.reload();
}
