<?php
declare(strict_types=1);
// Password through stdin keeps it out of process arguments and shell history.
if (PHP_SAPI !== "cli") {
    exit(1);
}
$path = $argv[1] ?? __DIR__ . "/../server/config.php";
if (file_exists($path)) {
    fwrite(STDERR, "Config already exists.\n");
    exit(1);
}
$password = rtrim(stream_get_contents(STDIN), "\r\n");
if (strlen($password) < 16) {
    fwrite(STDERR, "Use a password of at least 16 bytes on stdin.\n");
    exit(1);
}
$root = realpath(__DIR__ . "/..");
$config = [
    "password_hash" => password_hash($password, PASSWORD_DEFAULT),
    "storage" => $root . "/storage",
    "public_catalog" => $root . "/public/catalog",
    "seed" => $root . "/../assets/catalog/catalog-v1.json",
    "allow_local_http" => false,
];
if (getenv("LAUNCHPAD_LOCAL_SETUP") === "1") {
    $config["allow_local_http"] = true;
}
file_put_contents($path, "<?php\nreturn " . var_export($config, true) . ";\n");
chmod($path, 0600);
echo "Config created. Adjust storage, public_catalog and seed for the deployment.\n";
