<?php
declare(strict_types=1);
namespace Launchpad;

final class Icons
{
    public function __construct(
        private string $directory,
        private string $cache,
    ) {}
    private function fetch(string $url, int $limit): string
    {
        $ch = curl_init($url);
        $body = "";
        curl_setopt_array($ch, [
            CURLOPT_FOLLOWLOCATION => false,
            CURLOPT_CONNECTTIMEOUT => 4,
            CURLOPT_TIMEOUT => 10,
            CURLOPT_PROTOCOLS => CURLPROTO_HTTPS,
            CURLOPT_USERAGENT => "LaunchpadCatalog/1.0",
            CURLOPT_WRITEFUNCTION => function ($ch, $chunk) use (
                &$body,
                $limit,
            ) {
                if (strlen($body) + strlen($chunk) > $limit) {
                    return 0;
                }
                $body .= $chunk;
                return strlen($chunk);
            },
        ]);
        $ok = curl_exec($ch);
        $status = curl_getinfo($ch, CURLINFO_RESPONSE_CODE);
        if ($ok === false || $status !== 200) {
            throw new ApiError(
                502,
                "公開サービスから取得できませんでした。画像アップロードも利用できます。",
            );
        }
        return $body;
    }
    public function search(string $term, string $country): array
    {
        if (
            strlen($term) < 2 ||
            strlen($term) > 120 ||
            !in_array($country, ["jp", "us"], true)
        ) {
            throw new ApiError(
                422,
                "検索語（2文字以上）と国を確認してください。",
            );
        }
        $file =
            $this->cache .
            "/search-" .
            hash("sha256", $term . $country) .
            ".json";
        if (is_file($file) && filemtime($file) > time() - 86400) {
            return json_decode(
                (string) file_get_contents($file),
                true,
                64,
                JSON_THROW_ON_ERROR,
            );
        }
        // Fixed provider endpoint; arbitrary URL crawling is intentionally unsupported.
        $json = json_decode(
            $this->fetch(
                "https://itunes.apple.com/search?" .
                    http_build_query([
                        "term" => $term,
                        "country" => $country,
                        "entity" => "macSoftware",
                        "limit" => 12,
                    ]),
                1048576,
            ),
            true,
            64,
            JSON_THROW_ON_ERROR,
        );
        $results = [];
        foreach ($json["results"] ?? [] as $item) {
            $url = $item["artworkUrl512"] ?? ($item["artworkUrl100"] ?? "");
            if (!self::allowedArtwork($url)) {
                continue;
            }
            $results[] = [
                "name" => $item["trackName"],
                "bundle" => $item["bundleId"] ?? "",
                "category" => $item["primaryGenreName"] ?? "",
                "publisher" => $item["artistName"] ?? "",
                "artwork" => $url,
                "store_url" => $item["trackViewUrl"] ?? "",
            ];
        }
        CatalogStore::atomic($file, Contract::encode($results));
        return $results;
    }
    public static function allowedArtwork(string $url): bool
    {
        $p = parse_url($url);
        return is_array($p) &&
            ($p["scheme"] ?? "") === "https" &&
            preg_match('/^[a-z0-9-]+\.mzstatic\.com$/D', $p["host"] ?? "") ===
                1 &&
            !isset($p["user"]) &&
            !isset($p["pass"]) &&
            !isset($p["port"]) &&
            !isset($p["fragment"]);
    }
    public function import(string $url): array
    {
        if (!self::allowedArtwork($url)) {
            throw new ApiError(422, "App Storeの画像URLのみ取得できます。");
        }
        return $this->save($this->fetch($url, 2097152), $url);
    }
    public function save(string $bytes, string $source): array
    {
        if (strlen($bytes) > 2097152) {
            throw new ApiError(422, "画像は2 MiBまでです。");
        }
        $info = @getimagesizefromstring($bytes);
        if (
            !$info ||
            !in_array(
                $info[2],
                [IMAGETYPE_PNG, IMAGETYPE_JPEG, IMAGETYPE_WEBP],
                true,
            ) ||
            $info[0] > 2048 ||
            $info[1] > 2048 ||
            $info[0] < 1 ||
            $info[1] < 1
        ) {
            throw new ApiError(
                422,
                "2048px以下のPNG・JPEG・WebPを選んでください。",
            );
        }
        $image = @imagecreatefromstring($bytes);
        if (!$image) {
            throw new ApiError(422, "画像を読み込めません。");
        }
        $size = min(256, max($info[0], $info[1]));
        $out = imagecreatetruecolor($size, $size);
        imagealphablending($out, false);
        imagesavealpha($out, true);
        imagefill($out, 0, 0, imagecolorallocatealpha($out, 0, 0, 0, 127));
        $scale = $size / max($info[0], $info[1]);
        $w = (int) round($info[0] * $scale);
        $h = (int) round($info[1] * $scale);
        imagecopyresampled(
            $out,
            $image,
            (int) (($size - $w) / 2),
            (int) (($size - $h) / 2),
            0,
            0,
            $w,
            $h,
            $info[0],
            $info[1],
        );
        ob_start();
        imagepng($out);
        $png = ob_get_clean();
        $name = hash("sha256", $png) . ".png";
        CatalogStore::atomic($this->directory . "/" . $name, $png);
        return ["path" => "icons/" . $name, "source" => $source];
    }
}
