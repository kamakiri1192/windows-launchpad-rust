<?php
declare(strict_types=1);
namespace Launchpad;
final class Collector
{
    public function __construct(
        private string $storage,
        private CatalogStore $store,
        private Inbox $inbox,
        private Icons $icons,
    ) {}
    public function settings(): array
    {
        $p = $this->storage . "/collection-settings.json";
        return is_file($p)
            ? json_decode((string) file_get_contents($p), true)
            : [
                "terms" => ["productivity", "developer tools", "photo editor"],
                "country" => "us",
                "auto_accept" => false,
            ];
    }
    public function configure(array $settings): array
    {
        if (
            !is_array($settings["terms"] ?? null) ||
            count($settings["terms"]) > 10 ||
            !in_array($settings["country"] ?? "", ["jp", "us"], true) ||
            !is_bool($settings["auto_accept"] ?? null)
        ) {
            throw new ApiError(422, "収集設定が不正です。");
        }
        foreach ($settings["terms"] as $term) {
            if (!is_string($term) || strlen($term) < 2 || strlen($term) > 120) {
                throw new ApiError(
                    422,
                    "検索語は2～120文字で指定してください。",
                );
            }
        }
        $clean = [
            "terms" => array_values(array_unique($settings["terms"])),
            "country" => $settings["country"],
            "auto_accept" => $settings["auto_accept"],
        ];
        $this->store->locked(
            fn() => CatalogStore::atomic(
                $this->storage . "/collection-settings.json",
                Contract::encode($clean),
            ),
        );
        return $clean;
    }
    public function run(): array
    {
        // Independent lock: cron and browser cannot run the same collection together.
        $lock = fopen($this->storage . "/collection.lock", "c");
        if (!flock($lock, LOCK_EX | LOCK_NB)) {
            throw new ApiError(409, "収集はすでに実行中です。");
        }
        try {
            $settings = $this->settings();
            $added = 0;
            $errors = [];
            $deadline = microtime(true) + 15;
            $progressPath = $this->storage . "/collection-progress.json";
            $signature = hash("sha256", Contract::encode($settings));
            $progress = is_file($progressPath)
                ? json_decode((string) file_get_contents($progressPath), true)
                : null;
            if (($progress["signature"] ?? null) !== $signature) {
                $progress = [
                    "signature" => $signature,
                    "term" => 0,
                    "app" => 0,
                ];
            }
            $hasMore = false;
            foreach ($settings["terms"] as $termIndex => $term) {
                if ($termIndex < $progress["term"]) {
                    continue;
                }
                if (microtime(true) >= $deadline) {
                    $hasMore = true;
                    break;
                }
                try {
                    $apps = [];
                    foreach (
                        $this->icons->search($term, $settings["country"])
                        as $result
                    ) {
                        if (!$result["bundle"]) {
                            continue;
                        }
                        $apps[] = [
                            "name" => $result["name"],
                            "platform" => "macos",
                            "rule" => [
                                "type" => "mac_bundle",
                                "bundle" => $result["bundle"],
                                "product" => $result["bundle"],
                            ],
                            "category" => $result["category"],
                            "artwork" => $result["artwork"],
                        ];
                    }
                    // Individual failures don't lose successful candidates.
                    foreach ($apps as $appIndex => $app) {
                        if (
                            $termIndex === $progress["term"] &&
                            $appIndex < $progress["app"]
                        ) {
                            continue;
                        }
                        if (microtime(true) >= $deadline) {
                            $hasMore = true;
                            break 2;
                        }
                        try {
                            $added += $this->inbox->ingest([
                                "schema_version" => 1,
                                "source" => "app-store",
                                "apps" => [$app],
                            ])["added"];
                        } catch (\Throwable $e) {
                            $errors[] = $app["name"] . ": " . $e->getMessage();
                        }
                        $progress["term"] = $termIndex;
                        $progress["app"] = $appIndex + 1;
                        CatalogStore::atomic(
                            $progressPath,
                            Contract::encode($progress),
                        );
                    }
                } catch (\Throwable $e) {
                    $errors[] = $term . ": " . $e->getMessage();
                }
                $progress["term"] = $termIndex + 1;
                $progress["app"] = 0;
                CatalogStore::atomic(
                    $progressPath,
                    Contract::encode($progress),
                );
            }
            if (!$hasMore && is_file($progressPath)) {
                unlink($progressPath);
            }
            $accepted = 0;
            if ($settings["auto_accept"]) {
                $ids = [];
                foreach ($this->inbox->list() as $item) {
                    if (
                        $item["confidence"] === "high" &&
                        $item["rule"] &&
                        $item["folder_id"]
                    ) {
                        $ids[] = [
                            "id" => $item["id"],
                            "folder_id" => $item["folder_id"],
                        ];
                    }
                }
                if ($ids) {
                    try {
                        $accepted = $this->inbox->accept(
                            $ids,
                            $this->store->draft()["revision"],
                        )["accepted"];
                    } catch (\Throwable $e) {
                        $errors[] = $e->getMessage();
                    }
                }
            }
            $result = [
                "at" => gmdate("c"),
                "has_more" => $hasMore,
                "added" => $added,
                "accepted" => $accepted,
                "errors" => $errors,
            ];
            CatalogStore::atomic(
                $this->storage . "/collection-last.json",
                Contract::encode($result),
            );
            return $result;
        } finally {
            flock($lock, LOCK_UN);
            fclose($lock);
        }
    }
    public function status(): array
    {
        $path = $this->storage . "/collection-last.json";
        return [
            "settings" => $this->settings(),
            "last_run" => is_file($path)
                ? json_decode((string) file_get_contents($path), true)
                : null,
        ];
    }
}
