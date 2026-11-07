<?php
declare(strict_types=1);
if (PHP_SAPI !== "cli") {
    exit(1);
}
require __DIR__ . "/Contract.php";
require __DIR__ . "/CatalogStore.php";
require __DIR__ . "/Icons.php";
require __DIR__ . "/Inbox.php";
require __DIR__ . "/Collector.php";
$config = require getenv("LAUNCHPAD_ADMIN_CONFIG") ?: __DIR__ . "/config.php";
$store = new Launchpad\CatalogStore(
    $config["storage"],
    $config["public_catalog"],
    $config["seed"],
);
$icons = new Launchpad\Icons(
    $config["public_catalog"] . "/icons",
    $config["storage"],
);
$inbox = new Launchpad\Inbox($config["storage"], $store, $icons);
$collector = new Launchpad\Collector(
    $config["storage"],
    $store,
    $inbox,
    $icons,
);
$failed = false;
do {
    $result = $collector->run();
    echo Launchpad\Contract::encode($result);
    $failed = $failed || (bool) $result["errors"];
} while ($result["has_more"]);
exit($failed ? 1 : 0);
