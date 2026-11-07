<?php
declare(strict_types=1);
namespace Launchpad;

final class CatalogStore
{
    public function __construct(
        private string $storage,
        private string $public,
        private string $seed,
    ) {
        foreach (
            [$storage, "$storage/releases", $public, "$public/icons"]
            as $dir
        ) {
            if (!is_dir($dir) && !mkdir($dir, 0770, true) && !is_dir($dir)) {
                throw new \RuntimeException("Cannot create storage");
            }
        }
    }
    public static function atomic(string $path, string $bytes): void
    {
        $tmp = tempnam(dirname($path), ".pending-");
        if ($tmp === false) {
            throw new \RuntimeException("Cannot create temporary file");
        }
        try {
            if (
                file_put_contents($tmp, $bytes) !== strlen($bytes) ||
                !chmod($tmp, 0644) ||
                !rename($tmp, $path)
            ) {
                throw new \RuntimeException("Cannot commit file");
            }
        } finally {
            if (is_file($tmp)) {
                unlink($tmp);
            }
        }
    }
    public function locked(callable $fn): mixed
    {
        $lock = fopen("$this->storage/write.lock", "c");
        if (!$lock || !flock($lock, LOCK_EX)) {
            throw new \RuntimeException("Cannot lock storage");
        }
        try {
            return $fn();
        } finally {
            flock($lock, LOCK_UN);
            fclose($lock);
        }
    }
    private function read(string $path): array
    {
        return json_decode(
            (string) file_get_contents($path),
            true,
            64,
            JSON_THROW_ON_ERROR,
        );
    }
    public function draft(): array
    {
        $doc = $this->read(
            is_file("$this->storage/draft.json")
                ? "$this->storage/draft.json"
                : $this->seed,
        );
        return [
            "document" => $doc,
            "revision" => hash("sha256", Contract::encode($doc)),
        ];
    }
    public function save(array $doc, string $revision): array
    {
        Contract::validate($doc);
        return $this->locked(function () use ($doc, $revision) {
            $current = $this->draft();
            if (!hash_equals($current["revision"], $revision)) {
                throw new ApiError(
                    409,
                    "別の画面で更新されました。再読み込みして変更を確認してください。",
                );
            }
            // The server owns release numbers, never the browser.
            $doc["version"] = $current["document"]["version"];
            $doc["published_at"] = $current["document"]["published_at"];
            self::atomic("$this->storage/draft.json", Contract::encode($doc));
            return $this->draft();
        });
    }
    public function status(): array
    {
        $versions = [];
        foreach (glob("$this->storage/releases/*.json") ?: [] as $path) {
            $d = $this->read($path);
            $versions[] = [
                "version" => $d["version"],
                "published_at" => $d["published_at"],
                "apps" => count($d["apps"]),
                "folders" => count($d["folders"]),
            ];
        }
        usort($versions, fn($a, $b) => $b["version"] <=> $a["version"]);
        $live = is_file("$this->public/catalog-v1.json")
            ? $this->read("$this->public/catalog-v1.json")
            : null;
        return [
            "published" => $live
                ? [
                    "version" => $live["version"],
                    "published_at" => $live["published_at"],
                ]
                : null,
            "releases" => $versions,
            "changes" => self::changes($live, $this->draft()["document"]),
        ];
    }
    private static function changes(?array $live, array $draft): array
    {
        $old = array_column($live["apps"] ?? [], null, "id");
        $current = array_column($draft["apps"], null, "id");
        $diff = [
            "added" => [],
            "changed" => [],
            "removed" => [],
            "folders_changed" => ($live["folders"] ?? []) != $draft["folders"],
        ];
        foreach ($current as $id => $app) {
            if (!isset($old[$id])) {
                $diff["added"][] = ["id" => $id, "name" => $app["name"]];
            } elseif ($app != $old[$id]) {
                $diff["changed"][] = ["id" => $id, "name" => $app["name"]];
            }
        }
        foreach ($old as $id => $app) {
            if (!isset($current[$id])) {
                $diff["removed"][] = ["id" => $id, "name" => $app["name"]];
            }
        }
        return $diff;
    }
    public function publish(string $revision, ?int $restore = null): array
    {
        return $this->locked(function () use ($revision, $restore) {
            $current = $this->draft();
            if (!hash_equals($current["revision"], $revision)) {
                throw new ApiError(
                    409,
                    "下書きが更新されています。再読み込みしてください。",
                );
            }
            $doc = $current["document"];
            if ($restore !== null) {
                $file = "$this->storage/releases/$restore.json";
                if (!is_file($file)) {
                    throw new ApiError(404, "指定された公開版がありません。");
                }
                $doc = $this->read($file);
            }
            Contract::validate($doc);
            $versions = array_column($this->status()["releases"], "version");
            $doc["version"] = max([$doc["version"], ...$versions]) + 1;
            $doc["published_at"] = gmdate("Y-m-d\TH:i:s\Z");
            Contract::validate($doc);
            $bytes = Contract::encode($doc);
            // Immutable recovery snapshot precedes the atomic public pointer replacement.
            self::atomic(
                "$this->storage/releases/{$doc["version"]}.json",
                $bytes,
            );
            // Distribution is compact; human-readable recovery files remain private.
            self::atomic(
                "$this->public/catalog-v1.json",
                json_encode(
                    $doc,
                    JSON_UNESCAPED_UNICODE |
                        JSON_UNESCAPED_SLASHES |
                        JSON_THROW_ON_ERROR,
                ),
            );
            self::atomic("$this->storage/draft.json", $bytes);
            return ["draft" => $this->draft(), "status" => $this->status()];
        });
    }
}
