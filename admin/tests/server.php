<?php
declare(strict_types=1);
require __DIR__ . "/../server/Contract.php";
require __DIR__ . "/../server/CatalogStore.php";
require __DIR__ . "/../server/Icons.php";
require __DIR__ . "/../server/Inbox.php";
require __DIR__ . "/../server/TokenStore.php";
require __DIR__ . "/../server/McpServer.php";
use Launchpad\{Contract, CatalogStore, Icons, ApiError};
function check(bool $yes, string $message): void
{
    if (!$yes) {
        throw new RuntimeException($message);
    }
}
function rejects(callable $fn, int $status): void
{
    try {
        $fn();
    } catch (ApiError $e) {
        check($e->status === $status, $e->getMessage());
        return;
    }
    throw new RuntimeException("Expected rejection");
}
$seed = __DIR__ . "/../../assets/catalog/catalog-v1.json";
$root =
    sys_get_temp_dir() . "/launchpad-admin-test-" . bin2hex(random_bytes(8));
try {
    $store = new CatalogStore($root . "/private", $root . "/public", $seed);
    $draft = $store->draft();
    Contract::validate($draft["document"]);
    rejects(fn() => $store->save($draft["document"], "old"), 409);
    $bad = $draft["document"];
    $bad["apps"][0]["folder_id"] = "missing";
    rejects(fn() => Contract::validate($bad), 422);
    $bad = $draft["document"];
    $bad["schema_version"] = 2;
    rejects(fn() => Contract::validate($bad), 422);
    $bad = $draft["document"];
    $copy = $bad["apps"][1];
    $copy["id"] = "conflict";
    $copy["folder_id"] = "steam-library";
    $bad["apps"][] = $copy;
    rejects(fn() => Contract::validate($bad), 422);
    $published = $store->publish($draft["revision"]);
    check(
        $published["status"]["published"]["version"] === 8,
        "version increases",
    );
    $doc = $published["draft"]["document"];
    $publicBytes = file_get_contents($root . "/public/catalog-v1.json");
    check(
        json_decode($publicBytes, true) === $doc,
        "compact release preserves all data",
    );
    check(
        strlen($publicBytes) < strlen(Contract::encode($doc)),
        "public release omits formatting bytes",
    );
    $doc["apps"][0]["name"] = "Edited";
    $saved = $store->save($doc, $published["draft"]["revision"]);
    check(
        json_decode(file_get_contents($root . "/public/catalog-v1.json"), true)[
            "apps"
        ][0]["name"] !== "Edited",
        "draft stays private",
    );
    $second = $store->publish($saved["revision"]);
    $restored = $store->publish($second["draft"]["revision"], 8);
    check(
        $restored["status"]["published"]["version"] === 10,
        "rollback is a new monotonic release",
    );
    check(
        $restored["draft"]["document"]["apps"][0]["name"] !== "Edited",
        "rollback restores content",
    );
    foreach (
        [
            "http://is1.mzstatic.com/a",
            "https://localhost/a",
            "https://is1.mzstatic.com.evil.com/a",
            "https://user@is1.mzstatic.com/a",
            "https://is1.mzstatic.com:443/a",
        ]
        as $url
    ) {
        check(!Icons::allowedArtwork($url), "SSRF allowlist");
    }
    $icons = new Icons($root . "/public/icons", $root . "/private");
    rejects(fn() => $icons->save("<?php echo 1;", "upload"), 422);
    $im = imagecreatetruecolor(2, 2);
    ob_start();
    imagepng($im);
    $bytes = ob_get_clean();
    $icon = $icons->save($bytes, "upload");
    check(is_file($root . "/public/" . $icon["path"]), "sanitized icon saved");
    $inbox = new Launchpad\Inbox($root . "/private", $store, $icons);
    $batch = [
        "schema_version" => 1,
        "source" => "desktop",
        "apps" => [
            [
                "name" => "New Mac Editor",
                "platform" => "macos",
                "rule" => [
                    "type" => "mac_bundle",
                    "bundle" => "test.editor",
                    "product" => "test-editor",
                ],
                "category" => "public.app-category.developer-tools",
            ],
            [
                "name" => "New Windows Tool",
                "platform" => "windows",
                "rule" => [
                    "type" => "windows_metadata",
                    "executable" => "test.exe",
                    "publisher" => "Vendor",
                    "product_name" => "Test Tool",
                    "product" => "tool",
                ],
            ],
        ],
    ];
    $inbox->ingest($batch);
    $inbox->ingest($batch);
    check(count($inbox->list()) === 2, "repeat ingestion is deduplicated");
    $candidates = array_values($inbox->list());
    $mac = array_values(
        array_filter($candidates, fn($a) => $a["platform"] === "macos"),
    )[0];
    check($mac["confidence"] === "high", "official category is suggested");
    rejects(
        fn() => $inbox->accept(
            [["id" => $mac["id"], "folder_id" => "windows-tools"]],
            $store->draft()["revision"],
        ),
        422,
    );
    $before = $store->draft();
    $inbox->accept(
        [["id" => $mac["id"], "folder_id" => $mac["folder_id"]]],
        $before["revision"],
    );
    check(count($inbox->list()) === 1, "accepted entries leave queue");
    rejects(
        fn() => $store->save($before["document"], $before["revision"]),
        409,
    );
    $tokens = new Launchpad\TokenStore($root . "/private/token.json", $store);
    $first = $tokens->rotate();
    check($tokens->verify($first["token"]), "valid token");
    $second = $tokens->rotate();
    check(!$tokens->verify($first["token"]), "rotation revokes");
    check(
        !str_contains(
            file_get_contents($root . "/private/token.json"),
            $second["token"],
        ),
        "only hash stored",
    );
    $tokens->revoke();
    check(!$tokens->verify($second["token"]), "revocation");
    $mcp = new Launchpad\McpServer($store, $inbox);
    $reply = $mcp->handle([
        "jsonrpc" => "2.0",
        "id" => 1,
        "method" => "tools/call",
        "params" => [
            "name" => "save_catalog",
            "arguments" => ["document" => [], "revision" => "old"],
        ],
    ]);
    check($reply["result"]["isError"], "MCP domain errors are tool errors");
    check(
        $mcp->handle(["jsonrpc" => "2.0", "id" => 2, "method" => "bad"])[
            "error"
        ]["code"] === -32601,
        "MCP unknown method",
    );
    echo "Server tests passed: contract, conflicts, private drafts, publish, rollback, icon boundaries.\n";
} finally {
    $it = new RecursiveIteratorIterator(
        new RecursiveDirectoryIterator($root, FilesystemIterator::SKIP_DOTS),
        RecursiveIteratorIterator::CHILD_FIRST,
    );
    foreach ($it as $f) {
        $f->isDir() ? rmdir($f->getPathname()) : unlink($f->getPathname());
    }
    rmdir($root);
}
