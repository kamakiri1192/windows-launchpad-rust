<?php
declare(strict_types=1);
require __DIR__ . "/../server/Contract.php";
require __DIR__ . "/../server/CatalogStore.php";
require __DIR__ . "/../server/Icons.php";
require __DIR__ . "/../server/Inbox.php";
use Launchpad\{CatalogStore, Icons, Inbox, Contract};
if (PHP_SAPI !== "cli" || !isset($argv[1], $argv[2])) {
    exit(2);
}
$inventory = json_decode(
    file_get_contents($argv[1]),
    true,
    64,
    JSON_THROW_ON_ERROR,
);
$root = sys_get_temp_dir() . "/launchpad-inventory-" . bin2hex(random_bytes(8));
try {
    $store = new CatalogStore(
        $root . "/private",
        $root . "/public",
        __DIR__ . "/../../assets/catalog/catalog-v1.json",
    );
    $icons = new Icons($root . "/public/icons", $root . "/private");
    $inbox = new Inbox($root . "/private", $store, $icons);
    foreach (array_chunk($inventory["apps"], 100) as $apps) {
        $inbox->ingest([
            "schema_version" => 1,
            "source" => "desktop",
            "apps" => $apps,
        ]);
    }
    $candidates = $inbox->list();
    $ready = [];
    foreach ($candidates as $item) {
        if (
            $item["confidence"] === "high" &&
            $item["rule"] &&
            $item["folder_id"]
        ) {
            $ready[] = ["id" => $item["id"], "folder_id" => $item["folder_id"]];
        }
    }
    $saved = $inbox->accept($ready, $store->draft()["revision"]);
    $published = $store->publish($saved["draft"]["revision"]);
    file_put_contents(
        $argv[2],
        Contract::encode($published["draft"]["document"]),
    );
    echo json_encode([
        "input" => count($inventory["apps"]),
        "candidates" => count($candidates),
        "accepted" => count($ready),
        "remaining_review" => count($inbox->list()),
        "catalog_apps" => count($published["draft"]["document"]["apps"]),
    ]) . "\n";
} finally {
    if (is_dir($root)) {
        $it = new RecursiveIteratorIterator(
            new RecursiveDirectoryIterator(
                $root,
                FilesystemIterator::SKIP_DOTS,
            ),
            RecursiveIteratorIterator::CHILD_FIRST,
        );
        foreach ($it as $f) {
            $f->isDir() ? rmdir($f->getPathname()) : unlink($f->getPathname());
        }
        rmdir($root);
    }
}
