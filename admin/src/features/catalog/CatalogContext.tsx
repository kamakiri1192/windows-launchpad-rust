import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../../shared/api";
import { catalogSchema, type Catalog, type Draft } from "./schema";
type State = {
  document: Catalog;
  revision: string;
  dirty: boolean;
  update: (doc: Catalog) => void;
  save: () => Promise<void>;
  accept: (draft: Draft) => void;
  saving: boolean;
};
const Context = createContext<State | null>(null);
export function CatalogProvider({ children }: { children: ReactNode }) {
  const client = useQueryClient();
  const query = useQuery({
    queryKey: ["catalog"],
    queryFn: async () => {
      const d = await api<Draft>("catalog");
      return { ...d, document: catalogSchema.parse(d.document) };
    },
    retry: false,
    refetchOnWindowFocus: false,
  });
  const [local, setLocal] = useState<Draft | null>(null);
  const [saving, setSaving] = useState(false);
  const dirty =
    local !== null &&
    JSON.stringify(local.document) !== JSON.stringify(query.data?.document);
  useEffect(() => {
    const listener = (e: BeforeUnloadEvent) => {
      if (dirty) {
        e.preventDefault();
      }
    };
    window.addEventListener("beforeunload", listener);
    return () => window.removeEventListener("beforeunload", listener);
  }, [dirty]);
  const accept = (draft: Draft) => {
    client.setQueryData(["catalog"], draft);
    setLocal(null);
    void client.invalidateQueries({ queryKey: ["publication"] });
  };
  if (query.isPending)
    return <div className="loading-state">カタログを読み込んでいます…</div>;
  if (query.error)
    return (
      <div role="alert" className="loading-state">
        <h2>カタログを取得できません</h2>
        <p>{query.error.message}</p>
        <button onClick={() => void query.refetch()}>再試行</button>
      </div>
    );
  const doc = local?.document ?? query.data.document;
  const revision = local?.revision ?? query.data.revision;
  return (
    <Context.Provider
      value={{
        document: doc,
        revision,
        dirty,
        update: (document) =>
          setLocal((previous) => ({
            document,
            revision: previous?.revision ?? query.data.revision,
          })),
        accept,
        saving,
        save: async () => {
          setSaving(true);
          try {
            catalogSchema.parse(doc);
            const submitted = local;
            const saved = await api<Draft>("save", { document: doc, revision });
            client.setQueryData(["catalog"], saved);
            void client.invalidateQueries({ queryKey: ["publication"] });
            setLocal((current) =>
              current === submitted
                ? null
                : current
                  ? { ...current, revision: saved.revision }
                  : null,
            );
          } finally {
            setSaving(false);
          }
        },
      }}
    >
      {children}
    </Context.Provider>
  );
}
export function useCatalog() {
  const value = useContext(Context);
  if (!value) throw new Error("CatalogProvider missing");
  return value;
}
