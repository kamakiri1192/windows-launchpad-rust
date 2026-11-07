<?php
declare(strict_types=1);
if (PHP_SAPI !== "cli") {
    exit(1);
}
$root = sys_get_temp_dir() . "/launchpad-admin-e2e-" . bin2hex(random_bytes(8));
mkdir($root . "/public/api", 0700, true);
$server = realpath(__DIR__ . "/../server");
foreach (
    ["index.php" => "bootstrap.php", "mcp.php" => "mcp.php"]
    as $entry => $target
) {
    file_put_contents(
        $root . "/public/api/" . $entry,
        "<?php require " . var_export($server . "/" . $target, true) . ";",
    );
}
$config = [
    "password_hash" => password_hash(
        "launchpad-local-test-password",
        PASSWORD_DEFAULT,
    ),
    "storage" => $root . "/storage",
    "public_catalog" => $root . "/public/catalog",
    "seed" => realpath(__DIR__ . "/../../assets/catalog/catalog-v1.json"),
    "allow_local_http" => true,
];
file_put_contents(
    $root . "/config.php",
    "<?php return " . var_export($config, true) . ";",
);
chmod($root . "/config.php", 0600);
echo json_encode(
    [
        "root" => $root,
        "config" => $root . "/config.php",
        "public" => $root . "/public",
    ],
    JSON_THROW_ON_ERROR,
);
