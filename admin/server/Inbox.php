<?php
declare(strict_types=1);
namespace Launchpad;
final class Inbox
{
    public function __construct(
        private string $storage,
        private CatalogStore $store,
        private Icons $icons,
    ) {}
    public function list(): array
    {
        $path = $this->storage . "/inbox.json";
        return is_file($path)
            ? json_decode(
                (string) file_get_contents($path),
                true,
                64,
                JSON_THROW_ON_ERROR,
            )
            : [];
    }
    private static function identity(array $rule): string
    {
        unset($rule["product"]);
        ksort($rule);
        return json_encode($rule, JSON_THROW_ON_ERROR);
    }
    public static function suggest(
        ?string $category,
        string $name,
        string $platform,
    ): array {
        $category = strtolower($category ?? "");
        $categories = [
            "developer-tools" => "macos-development",
            "developertools" => "macos-development",
            "productivity" => "macos-productivity",
            "business" => "macos-productivity",
            "graphics-design" => "macos-design",
            "graphics&design" => "macos-design",
            "photography" => "macos-image",
            "photo&video" => "macos-image",
            "video" => "macos-video-live",
            "music" => "macos-music-production",
            "social-networking" => "macos-communication",
            "socialnetworking" => "macos-communication",
            "games" => "macos-games",
            "utilities" => "macos-utilities",
            "education" => "macos-information",
            "weather" => "macos-information",
            "news" => "macos-information",
        ];
        $compact = preg_replace(
            "/\s+/",
            "",
            str_replace("public.app-category.", "", $category),
        );
        if ($platform === "macos" && isset($categories[$compact])) {
            return [$categories[$compact], "high", "OS・ストアの公式カテゴリ"];
        }
        // Name hints are proposals only; never automatically accepted.
        $hints = [
            "/editor|studio|developer|code|terminal/i" => "macos-development",
            "/photo|image|paint/i" => "macos-image",
            "/chat|discord|slack|zoom/i" => "macos-communication",
            "/video|movie|stream/i" => "macos-video-live",
        ];
        if ($platform === "macos") {
            foreach ($hints as $pattern => $folder) {
                if (preg_match($pattern, $name)) {
                    return [
                        $folder,
                        "review",
                        "アプリ名からの参考候補・要確認",
                    ];
                }
            }
        }
        return [null, "review", "分類先の確認が必要"];
    }
    public function ingest(array $inventory): array
    {
        if (
            ($inventory["schema_version"] ?? null) !== 1 ||
            !is_array($inventory["apps"] ?? null) ||
            count($inventory["apps"]) > 100
        ) {
            throw new ApiError(
                422,
                "1回100件以内のインベントリを指定してください。",
            );
        }
        $draft = $this->store->draft()["document"];
        $pending = $this->list();
        $known = [];
        foreach ($draft["apps"] as $app) {
            foreach ($app["rules"] as $rule) {
                $known[self::identity($rule)] = $app;
            }
        }
        $new = [];
        foreach ($inventory["apps"] as $item) {
            if (
                !is_array($item) ||
                !is_string($item["name"] ?? null) ||
                strlen($item["name"]) > 160 ||
                trim($item["name"]) === "" ||
                !in_array($item["platform"] ?? "", ["windows", "macos"], true)
            ) {
                throw new ApiError(422, "収集アプリの名前・OSが不正です。");
            }
            if ($item["keep_top_level"] ?? false) {
                continue;
            }
            $rule = $item["rule"] ?? null;
            if ($rule !== null) {
                if (!is_array($rule)) {
                    throw new ApiError(422, "識別ルールが不正です。");
                }
                $sample = $draft;
                $sample["apps"] = [
                    [
                        "id" => "validation",
                        "name" => $item["name"],
                        "folder_id" => array_values(
                            array_filter(
                                $draft["folders"],
                                fn($f) => in_array(
                                    $item["platform"],
                                    $f["platforms"],
                                    true,
                                ),
                            ),
                        )[0]["id"],
                        "enabled" => true,
                        "rules" => [$rule],
                        "icon" => null,
                    ],
                ];
                Contract::validate($sample);
            }
            $identity = $rule ? self::identity($rule) : $item["name"];
            $id = substr(hash("sha256", $item["platform"] . $identity), 0, 24);
            $existing = $known[$identity] ?? null;
            [$folder, $confidence, $reason] = self::suggest(
                is_string($item["category"] ?? null) ? $item["category"] : null,
                $item["name"],
                $item["platform"],
            );
            $suggested = $item["suggested_folder_id"] ?? null;
            if (
                is_string($suggested) &&
                in_array(
                    $suggested,
                    array_column($draft["folders"], "id"),
                    true,
                )
            ) {
                $folder = $suggested;
                $confidence = "high";
                $reason = "端末の既存分類ルール";
            }
            if ($existing) {
                $folder = $existing["folder_id"];
                $confidence = "high";
                $reason = "既存の識別ルールと一致";
            }
            if ($existing && $existing["icon"] !== null) {
                continue;
            }
            $icon = $pending[$id]["icon"] ?? null;
            if (isset($item["icon_png"])) {
                if (
                    !is_string($item["icon_png"]) ||
                    strlen($item["icon_png"]) > 2800000
                ) {
                    throw new ApiError(422, "画像が大きすぎます。");
                }
                $bytes = base64_decode($item["icon_png"], true);
                if ($bytes === false) {
                    throw new ApiError(422, "画像の形式が不正です。");
                }
                $icon = $this->icons->save($bytes, "Administrator inventory");
            } elseif (
                $icon === null &&
                isset($item["artwork"]) &&
                is_string($item["artwork"])
            ) {
                $icon = $this->icons->import($item["artwork"]);
            }
            // Existing complete records need no further work; icons may enrich empty records.
            if ($existing && ($existing["icon"] !== null || $icon === null)) {
                continue;
            }
            $new[$id] = [
                "id" => $id,
                "name" => $item["name"],
                "platform" => $item["platform"],
                "rule" => $rule,
                "folder_id" => $folder,
                "confidence" => $confidence,
                "reason" => $reason,
                "icon" => $icon,
                "existing_id" => $existing["id"] ?? null,
                "source" =>
                    ($inventory["source"] ?? "") === "app-store"
                        ? "App Store"
                        : "管理者の端末",
                "collected_at" => gmdate("c"),
            ];
        }
        return $this->store->locked(function () use ($new) {
            $inbox = $this->list();
            foreach ($new as $id => $item) {
                $inbox[$id] = $item;
            }
            if (count($inbox) > 10000) {
                throw new ApiError(
                    422,
                    "収集候補の上限です。先に確認してください。",
                );
            }
            CatalogStore::atomic(
                $this->storage . "/inbox.json",
                Contract::encode($inbox),
            );
            return ["added" => count($new), "pending" => count($inbox)];
        });
    }
    public function accept(array $selections, string $revision): array
    {
        if (count($selections) > 1000) {
            throw new ApiError(422, "1回1000件までです。");
        }
        $inbox = $this->list();
        $draft = $this->store->draft();
        $doc = $draft["document"];
        $accepted = [];
        foreach ($selections as $selection) {
            $id = $selection["id"] ?? "";
            if (!is_string($id) || !isset($inbox[$id])) {
                throw new ApiError(
                    409,
                    "収集候補が変更されています。再読み込みしてください。",
                );
            }
            $item = $inbox[$id];
            $folder = $selection["folder_id"] ?? $item["folder_id"];
            $definition = null;
            foreach ($doc["folders"] as $f) {
                if ($f["id"] === $folder) {
                    $definition = $f;
                }
            }
            if (
                !$definition ||
                !in_array($item["platform"], $definition["platforms"], true) ||
                !$item["rule"]
            ) {
                throw new ApiError(
                    422,
                    "識別情報と対象OSに合う分類先を確認してください。",
                );
            }
            $found = false;
            foreach ($doc["apps"] as &$app) {
                if ($app["id"] === ($item["existing_id"] ?? null)) {
                    if ($app["icon"] === null) {
                        $app["icon"] = $item["icon"];
                    }
                    $found = true;
                    break;
                }
                foreach ($app["rules"] as $rule) {
                    if (
                        self::identity($rule) === self::identity($item["rule"])
                    ) {
                        $found = true;
                        break 2;
                    }
                }
            }
            unset($app);
            if (!$found) {
                $doc["apps"][] = [
                    "id" => "collected-" . $id,
                    "name" => $item["name"],
                    "folder_id" => $folder,
                    "enabled" => true,
                    "rules" => [$item["rule"]],
                    "icon" => $item["icon"],
                ];
            }
            $accepted[] = $id;
        }
        // Optimistic revision protects manual/MCP edits; failed validation leaves the inbox intact.
        $saved = $this->store->save($doc, $revision);
        $this->store->locked(function () use ($accepted, $inbox) {
            $current = $this->list();
            foreach ($accepted as $id) {
                if (($current[$id] ?? null) === ($inbox[$id] ?? null)) {
                    unset($current[$id]);
                }
            }
            CatalogStore::atomic(
                $this->storage . "/inbox.json",
                Contract::encode($current),
            );
        });
        return ["draft" => $saved, "accepted" => count($accepted)];
    }
    public function dismiss(array $ids): void
    {
        $this->store->locked(function () use ($ids) {
            $items = $this->list();
            foreach ($ids as $id) {
                if (is_string($id)) {
                    unset($items[$id]);
                }
            }
            CatalogStore::atomic(
                $this->storage . "/inbox.json",
                Contract::encode($items),
            );
        });
    }
}
