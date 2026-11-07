import { lazy, Suspense, useState } from "react";
import { NavLink, Route, Routes, Navigate } from "react-router-dom";
import { Button, Badge } from "@radix-ui/themes";
import {
  Inbox,
  Layers3,
  LayoutGrid,
  FolderOpen,
  CloudUpload,
  Cable,
  LogOut,
  Save,
  Menu,
} from "lucide-react";
import {
  CatalogProvider,
  useCatalog,
} from "../features/catalog/CatalogContext";
import { Auth, logout } from "../features/auth/Auth";
import { message } from "../shared/api";
import { Notice } from "../shared/ui";
const Collection = lazy(() => import("../features/collection/CollectionPage"));
const Apps = lazy(() => import("../features/catalog/AppsPage"));
const Folders = lazy(() => import("../features/catalog/FoldersPage"));
const Publication = lazy(
  () => import("../features/publication/PublicationPage"),
);
const Integrations = lazy(
  () => import("../features/integrations/IntegrationsPage"),
);
const navigation = [
  { path: "/collection", label: "収集・確認", icon: Inbox },
  { path: "/apps", label: "アプリカタログ", icon: LayoutGrid },
  { path: "/folders", label: "分類フォルダ", icon: FolderOpen },
  { path: "/publication", label: "公開と履歴", icon: CloudUpload },
  { path: "/integrations", label: "MCP接続", icon: Cable },
];
function Shell() {
  const { dirty, save, saving, document } = useCatalog();
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [mobile, setMobile] = useState(false);
  return (
    <div className="app-shell">
      <aside className={mobile ? "sidebar open" : "sidebar"}>
        <NavLink to="/apps" className="brand">
          <span className="brand-mark">
            <Layers3 size={22} />
          </span>
          <span>
            Liquid Start<small>CONSOLE</small>
          </span>
        </NavLink>
        <div className="workspace-label">
          <span className="workspace-avatar">L</span>
          <div>
            Liquid Start 管理<small>オーナーワークスペース</small>
          </div>
          <Badge color="gray">管理者</Badge>
        </div>
        <div className="nav-label">WORKSPACE</div>
        <nav>
          {navigation.map((n) => (
            <NavLink key={n.path} to={n.path} onClick={() => setMobile(false)}>
              <n.icon size={18} />
              {n.label}
            </NavLink>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <div className="platform-note">
            <span className="status-dot enabled" />
            Windows & Mac<small>ひとつのカタログで、両方へ。</small>
          </div>
          <button
            onClick={() => void logout().catch((e) => setError(message(e)))}
          >
            <LogOut size={16} />
            ログアウト
          </button>
        </div>
      </aside>
      <div className="main-shell">
        <header className="topbar">
          <button
            className="mobile-menu icon-button"
            aria-label="メニュー"
            onClick={() => setMobile(!mobile)}
          >
            <Menu size={20} />
          </button>
          <span>
            ワークスペース <span className="slash">/</span> アプリ管理
          </span>
          <div>
            <span className="save-status">
              <span className={`status-dot ${dirty ? "" : "enabled"}`} />
              {dirty ? "未保存の変更" : "すべて保存済み"}
            </span>
            <Button
              size="2"
              variant={dirty ? "solid" : "soft"}
              disabled={!dirty || saving}
              onClick={async () => {
                setError("");
                setSuccess("");
                try {
                  await save();
                  setSuccess(
                    "下書きを保存しました。公開画面から配信できます。",
                  );
                } catch (e) {
                  setError(message(e));
                }
              }}
            >
              <Save size={14} />
              {saving ? "保存中…" : "下書きを保存"}
            </Button>
            <span className="owner-avatar">O</span>
          </div>
        </header>
        <main className="main-content">
          <Notice error={error} success={success} />
          <Suspense fallback={<p>読み込んでいます…</p>}>
            <Routes>
              <Route path="/collection" element={<Collection />} />
              <Route path="/apps" element={<Apps />} />
              <Route path="/folders" element={<Folders />} />
              <Route path="/publication" element={<Publication />} />
              <Route path="/integrations" element={<Integrations />} />
              <Route path="*" element={<Navigate to="/apps" replace />} />
            </Routes>
          </Suspense>
          <footer className="app-footer">
            <span>Liquid Start Console</span>
            <span>Catalog v{document.version} · Schema 1</span>
          </footer>
        </main>
      </div>
    </div>
  );
}
export default function App() {
  return (
    <Auth>
      <CatalogProvider>
        <Shell />
      </CatalogProvider>
    </Auth>
  );
}
