<?php
declare(strict_types=1);
namespace Launchpad;

final class ApiError extends \RuntimeException
{
    public function __construct(public readonly int $status, string $message)
    {
        parent::__construct($message);
    }
}
final class Contract
{
    public const MAX_BYTES = 2097152;
    public const FIELDS = [
        "mac_bundle" => ["bundle", "product"],
        "mac_versioned_bundle" => ["prefix", "product"],
        "mac_bundle_in_directory" => ["bundle", "directory", "product"],
        "mac_corel_suite" => [],
        "windows_package" => ["aumid", "product"],
        "steam_app" => ["id"],
        "windows_product" => [
            "executable",
            "directory",
            "product",
            "versioned_executable",
        ],
        "windows_metadata" => [
            "executable",
            "publisher",
            "product_name",
            "product",
        ],
        "windows_system" => ["group"],
        "related_windows" => ["group"],
        "windows_office" => [],
        "visual_studio" => [],
        "libre_office" => [],
        "jet_brains" => [],
        "adobe_creative" => [],
        "steam" => [],
    ];
    public static function fail(string $message): never
    {
        throw new ApiError(422, $message);
    }
    private static function keys(array $value, array $keys): void
    {
        if (
            array_diff(array_keys($value), $keys) ||
            array_diff($keys, array_keys($value))
        ) {
            self::fail("データの項目が一致しません。");
        }
    }
    private static function text(mixed $v, int $max): bool
    {
        return is_string($v) &&
            trim($v) !== "" &&
            strlen($v) <= $max &&
            !preg_match('/[\x00-\x1f\x7f]/u', $v);
    }
    private static function id(mixed $v): bool
    {
        return is_string($v) && preg_match('/^[a-z0-9-]{1,160}$/D', $v) === 1;
    }
    public static function validate(array $doc): array
    {
        self::keys($doc, [
            "schema_version",
            "version",
            "published_at",
            "folders",
            "apps",
        ]);
        if (
            $doc["schema_version"] !== 1 ||
            !is_int($doc["version"]) ||
            $doc["version"] < 1 ||
            $doc["version"] > 4294967295 ||
            !is_array($doc["folders"]) ||
            !array_is_list($doc["folders"]) ||
            count($doc["folders"]) < 1 ||
            count($doc["folders"]) > 200 ||
            !is_array($doc["apps"]) ||
            !array_is_list($doc["apps"]) ||
            count($doc["apps"]) > 10000 ||
            ($doc["published_at"] !== null &&
                !self::text($doc["published_at"], 64))
        ) {
            self::fail("カタログ形式またはサイズが不正です。");
        }
        $folders = [];
        foreach ($doc["folders"] as $f) {
            if (!is_array($f)) {
                self::fail("フォルダ形式が不正です。");
            }
            self::keys($f, ["id", "names", "platforms", "evidence"]);
            if (
                !self::id($f["id"]) ||
                isset($folders[$f["id"]]) ||
                !is_array($f["names"]) ||
                count($f["names"]) > 32 ||
                !self::text($f["names"]["ja"] ?? null, 160) ||
                !self::text($f["names"]["en"] ?? null, 160) ||
                !self::text($f["evidence"], 2048) ||
                !is_array($f["platforms"]) ||
                !array_is_list($f["platforms"]) ||
                !count($f["platforms"]) ||
                count($f["platforms"]) > 2 ||
                array_diff($f["platforms"], ["windows", "macos"])
            ) {
                self::fail("フォルダID・名称・対象OSを確認してください。");
            }
            foreach ($f["names"] as $locale => $name) {
                if (!self::text($locale, 32) || !self::text($name, 160)) {
                    self::fail("翻訳名が不正です。");
                }
            }
            $folders[$f["id"]] = $f["platforms"];
        }
        $ids = [];
        $matches = [];
        foreach ($doc["apps"] as $app) {
            if (!is_array($app)) {
                self::fail("アプリ形式が不正です。");
            }
            self::keys($app, [
                "id",
                "name",
                "folder_id",
                "enabled",
                "rules",
                "icon",
            ]);
            if (
                !self::id($app["id"]) ||
                isset($ids[$app["id"]]) ||
                !isset($folders[$app["folder_id"] ?? ""]) ||
                !self::text($app["name"], 160) ||
                !is_bool($app["enabled"]) ||
                !is_array($app["rules"]) ||
                !array_is_list($app["rules"]) ||
                !count($app["rules"]) ||
                count($app["rules"]) > 100
            ) {
                self::fail("アプリID・名前・分類先を確認してください。");
            }
            $ids[$app["id"]] = true;
            if ($app["icon"] !== null) {
                if (!is_array($app["icon"])) {
                    self::fail("アイコン形式が不正です。");
                }
                self::keys($app["icon"], ["path", "source"]);
                if (
                    !is_string($app["icon"]["path"]) ||
                    !preg_match(
                        '/^icons\/[a-f0-9]{64}\.png$/D',
                        $app["icon"]["path"],
                    ) ||
                    !self::text($app["icon"]["source"], 2048)
                ) {
                    self::fail("アイコン参照が不正です。");
                }
            }
            foreach ($app["rules"] as $rule) {
                if (
                    !is_array($rule) ||
                    !is_string($rule["type"] ?? null) ||
                    !array_key_exists($rule["type"], self::FIELDS)
                ) {
                    self::fail("対応していないルールです。");
                }
                self::keys($rule, ["type", ...self::FIELDS[$rule["type"]]]);
                foreach ($rule as $key => $value) {
                    if (
                        $key === "versioned_executable"
                            ? !is_bool($value)
                            : !self::text($value, 512)
                    ) {
                        self::fail("ルールの値を確認してください。");
                    }
                }
                $required = str_starts_with($rule["type"], "mac_")
                    ? "macos"
                    : (str_starts_with($rule["type"], "steam")
                        ? null
                        : "windows");
                if (
                    $required !== null &&
                    !in_array($required, $folders[$app["folder_id"]], true)
                ) {
                    self::fail(
                        "識別ルールと分類フォルダの対象OSが一致しません: " .
                            $app["name"],
                    );
                }
                $product = $rule["product"] ?? null;
                unset($rule["product"]);
                ksort($rule);
                $key = json_encode($rule, JSON_THROW_ON_ERROR);
                $result = [$app["folder_id"], $product];
                if (
                    $app["enabled"] &&
                    isset($matches[$key]) &&
                    $matches[$key] !== $result
                ) {
                    self::fail(
                        "同じ識別ルールが異なる分類先・製品に重複しています: " .
                            $app["name"],
                    );
                }
                if ($app["enabled"]) {
                    $matches[$key] = $result;
                }
            }
        }
        if (strlen(self::encode($doc)) > self::MAX_BYTES) {
            self::fail("カタログは2 MiBまでです。");
        }
        return $doc;
    }
    public static function encode(array $doc): string
    {
        return json_encode(
            $doc,
            JSON_PRETTY_PRINT |
                JSON_UNESCAPED_UNICODE |
                JSON_UNESCAPED_SLASHES |
                JSON_THROW_ON_ERROR,
        ) . "\n";
    }
}
