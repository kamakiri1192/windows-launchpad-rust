import { z } from "zod";
const text = z.string().trim().min(1).max(512);
export const ruleFields = {
  mac_bundle: ["bundle", "product"],
  mac_versioned_bundle: ["prefix", "product"],
  mac_bundle_in_directory: ["bundle", "directory", "product"],
  mac_corel_suite: [],
  windows_package: ["aumid", "product"],
  steam_app: ["id"],
  windows_product: [
    "executable",
    "directory",
    "product",
    "versioned_executable",
  ],
  windows_metadata: ["executable", "publisher", "product_name", "product"],
  windows_system: ["group"],
  related_windows: ["group"],
  windows_office: [],
  visual_studio: [],
  libre_office: [],
  jet_brains: [],
  adobe_creative: [],
  steam: [],
} as const;
export type RuleKind = keyof typeof ruleFields;
export type Rule = { type: RuleKind; [key: string]: string | boolean };
export const ruleLabels: Record<RuleKind, string> = {
  mac_bundle: "Mac · Bundle ID",
  mac_versioned_bundle: "Mac · バージョン付きBundle ID",
  mac_bundle_in_directory: "Mac · Bundle ID＋フォルダ",
  mac_corel_suite: "Mac · CorelDRAWスイート",
  windows_package: "Windows · AUMID",
  steam_app: "Steam · アプリID",
  windows_product: "Windows · 実行ファイル＋フォルダ",
  windows_metadata: "Windows · 実行ファイル＋製品情報",
  windows_system: "Windows · システム判定",
  related_windows: "Windows · 製品別判定",
  windows_office: "Windows · Office",
  visual_studio: "Windows · Visual Studio",
  libre_office: "Windows · LibreOffice",
  jet_brains: "Windows · JetBrains",
  adobe_creative: "Windows · Adobe",
  steam: "Steam · ライブラリ",
};
const ruleSchema = z.custom<Rule>((v) => {
  if (
    !v ||
    typeof v !== "object" ||
    !("type" in v) ||
    typeof v.type !== "string" ||
    !Object.hasOwn(ruleFields, v.type)
  )
    return false;
  const fields = ruleFields[v.type as RuleKind] as readonly string[];
  const record = v as Record<string, unknown>;
  return (
    Object.keys(record).length === fields.length + 1 &&
    fields.every((f) =>
      f === "versioned_executable"
        ? typeof record[f] === "boolean"
        : text.safeParse(record[f]).success,
    )
  );
}, "識別ルールの必須項目を入力してください。");
const id = z.string().regex(/^[a-z0-9-]{1,160}$/);
export const folderSchema = z.strictObject({
  id,
  names: z
    .record(z.string(), z.string().min(1).max(160))
    .refine((v) => v.ja && v.en, "日本語名・英語名は必須です。"),
  platforms: z
    .array(z.enum(["windows", "macos"]))
    .min(1)
    .max(2),
  evidence: z.string().min(1).max(2048),
});
export const appSchema = z.strictObject({
  id,
  name: z.string().min(1).max(160),
  folder_id: id,
  enabled: z.boolean(),
  rules: z.array(ruleSchema).min(1).max(100),
  icon: z
    .strictObject({
      path: z.string().regex(/^icons\/[a-f0-9]{64}\.png$/),
      source: z.string().min(1).max(2048),
    })
    .nullable(),
});
export const catalogSchema = z.strictObject({
  schema_version: z.literal(1),
  version: z.number().int().positive(),
  published_at: z.string().nullable(),
  folders: z.array(folderSchema).min(1).max(200),
  apps: z.array(appSchema).max(10000),
});
export type Catalog = z.infer<typeof catalogSchema>;
export type Folder = z.infer<typeof folderSchema>;
export type CatalogApp = z.infer<typeof appSchema>;
export type Draft = { document: Catalog; revision: string };
export type Release = {
  version: number;
  published_at: string;
  apps: number;
  folders: number;
};
export type Publication = {
  changes: {
    added: { id: string; name: string }[];
    changed: { id: string; name: string }[];
    removed: { id: string; name: string }[];
    folders_changed: boolean;
  };
  published: Pick<Release, "version" | "published_at"> | null;
  releases: Release[];
};
export function platforms(app: CatalogApp, folders: Folder[]) {
  const supported =
    folders.find((f) => f.id === app.folder_id)?.platforms ?? [];
  return supported.filter((os) =>
    app.rules.some((r) =>
      r.type.startsWith("mac_")
        ? os === "macos"
        : r.type.startsWith("steam")
          ? true
          : os === "windows",
    ),
  );
}
