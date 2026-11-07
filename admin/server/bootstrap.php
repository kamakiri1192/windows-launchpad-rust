<?php
declare(strict_types=1);
namespace Launchpad;
ini_set("display_errors", "0");
require __DIR__ . "/Contract.php";
require __DIR__ . "/CatalogStore.php";
require __DIR__ . "/Icons.php";
require __DIR__ . "/TokenStore.php";
require __DIR__ . "/Inbox.php";
require __DIR__ . "/Collector.php";

function respond(mixed $body, int $status = 200): never
{
    http_response_code($status);
    header("Content-Type: application/json; charset=utf-8");
    echo json_encode($body, JSON_UNESCAPED_UNICODE | JSON_THROW_ON_ERROR);
    exit();
}
function body(): array
{
    $raw = file_get_contents(
        "php://input",
        false,
        null,
        0,
        Contract::MAX_BYTES + 1,
    );
    if (strlen($raw) > Contract::MAX_BYTES) {
        throw new ApiError(413, "リクエストが大きすぎます。");
    }
    try {
        $data = json_decode($raw, true, 64, JSON_THROW_ON_ERROR);
    } catch (\JsonException) {
        throw new ApiError(400, "JSONを読み込めません。");
    }
    if (!is_array($data)) {
        throw new ApiError(400, "JSONオブジェクトを指定してください。");
    }
    return $data;
}
try {
    header("Cache-Control: no-store");
    header("X-Content-Type-Options: nosniff");
    $configPath = getenv("LAUNCHPAD_ADMIN_CONFIG") ?: __DIR__ . "/config.php";
    if (!is_file($configPath)) {
        throw new ApiError(
            503,
            "管理画面の初期設定が必要です。READMEのセットアップ手順を実行してください。",
        );
    }
    $config = require $configPath;
    $secure = ($_SERVER["HTTPS"] ?? "") === "on";
    $local =
        in_array($_SERVER["REMOTE_ADDR"] ?? "", ["127.0.0.1", "::1"], true) &&
        ($config["allow_local_http"] ?? false);
    if (!$secure && !$local) {
        throw new ApiError(403, "HTTPSでアクセスしてください。");
    }
    $storage = $config["storage"];
    $public = $config["public_catalog"];
    $store = new CatalogStore($storage, $public, $config["seed"]);
    ini_set("session.use_strict_mode", "1");
    ini_set("session.use_only_cookies", "1");
    session_save_path($storage);
    session_name("launchpad_admin");
    session_set_cookie_params([
        "lifetime" => 0,
        "path" => rtrim(dirname(dirname($_SERVER["SCRIPT_NAME"])), "/") . "/",
        "secure" => $secure,
        "httponly" => true,
        "samesite" => "Strict",
    ]);
    session_start();
    $_SESSION["csrf"] ??= bin2hex(random_bytes(32));
    if (($_SESSION["expires"] ?? 0) < time()) {
        unset($_SESSION["owner"]);
    }
    $action = $_GET["action"] ?? "session";
    $method = $_SERVER["REQUEST_METHOD"];
    if ($method === "GET" && $action === "session") {
        respond([
            "authenticated" => isset($_SESSION["owner"]),
            "csrf" => $_SESSION["csrf"],
        ]);
    }
    if ($method === "POST") {
        if (
            !hash_equals($_SESSION["csrf"], $_SERVER["HTTP_X_CSRF_TOKEN"] ?? "")
        ) {
            throw new ApiError(
                403,
                "セッションが更新されました。画面を再読み込みしてください。",
            );
        }
        if ($action === "login") {
            $data = body();
            $store->locked(function () use ($storage, $data, $config) {
                $path = $storage . "/login-attempts.json";
                $attempts = is_file($path)
                    ? json_decode((string) file_get_contents($path), true)
                    : [];
                $attempts = array_values(
                    array_filter($attempts, fn($t) => $t > time() - 900),
                );
                if (count($attempts) >= 10) {
                    throw new ApiError(
                        429,
                        "ログイン試行が多いため、15分後に再試行してください。",
                    );
                }
                $password = $data["password"] ?? "";
                if (
                    !is_string($password) ||
                    strlen($password) > 1024 ||
                    !password_verify($password, $config["password_hash"])
                ) {
                    $attempts[] = time();
                    CatalogStore::atomic($path, Contract::encode($attempts));
                    throw new ApiError(401, "パスワードが正しくありません。");
                }
                CatalogStore::atomic($path, "[]");
            });
            session_regenerate_id(true);
            $_SESSION["owner"] = true;
            $_SESSION["expires"] = time() + 28800;
            $_SESSION["csrf"] = bin2hex(random_bytes(32));
            respond(["authenticated" => true, "csrf" => $_SESSION["csrf"]]);
        }
    }
    if (!isset($_SESSION["owner"])) {
        throw new ApiError(401, "ログインしてください。");
    }
    if ($method === "POST" && $action === "logout") {
        $_SESSION = [];
        session_destroy();
        respond(["ok" => true]);
    }
    if ($method === "GET" && $action === "catalog") {
        respond($store->draft());
    }
    if ($method === "GET" && $action === "status") {
        respond($store->status());
    }
    if ($method === "POST" && $action === "save") {
        $data = body();
        respond(
            $store->save(
                $data["document"] ?? [],
                (string) ($data["revision"] ?? ""),
            ),
        );
    }
    if ($method === "POST" && in_array($action, ["publish", "restore"], true)) {
        $data = body();
        $version = $action === "restore" ? $data["version"] ?? 0 : null;
        if ($version !== null && (!is_int($version) || $version < 1)) {
            throw new ApiError(422, "公開版を指定してください。");
        }
        respond($store->publish((string) ($data["revision"] ?? ""), $version));
    }
    $tokens = new TokenStore($storage . "/mcp-token.json", $store);
    if ($method === "GET" && $action === "mcp_status") {
        respond($tokens->status());
    }
    if ($method === "POST" && $action === "mcp_rotate") {
        respond($tokens->rotate());
    }
    if ($method === "POST" && $action === "mcp_revoke") {
        $tokens->revoke();
        respond(["active" => false]);
    }
    $icons = new Icons($public . "/icons", $storage);
    $inbox = new Inbox($storage, $store, $icons);
    $collector = new Collector($storage, $store, $inbox, $icons);
    if ($method === "GET" && $action === "inbox") {
        respond(array_values($inbox->list()));
    }
    if ($method === "POST" && $action === "ingest") {
        respond($inbox->ingest(body()));
    }
    if ($method === "POST" && $action === "accept_inbox") {
        $data = body();
        respond(
            $inbox->accept(
                $data["selections"] ?? [],
                (string) ($data["revision"] ?? ""),
            ),
        );
    }
    if ($method === "POST" && $action === "dismiss_inbox") {
        $data = body();
        $inbox->dismiss($data["ids"] ?? []);
        respond(["ok" => true]);
    }
    if ($method === "GET" && $action === "collection") {
        respond($collector->status());
    }
    if ($method === "POST" && $action === "configure_collection") {
        respond($collector->configure(body()));
    }
    if ($method === "POST" && $action === "collect") {
        respond($collector->run());
    }
    if ($method === "GET" && $action === "icon_search") {
        respond(
            $icons->search(
                (string) ($_GET["term"] ?? ""),
                (string) ($_GET["country"] ?? "jp"),
            ),
        );
    }
    if ($method === "POST" && $action === "icon_import") {
        $data = body();
        respond($icons->import((string) ($data["url"] ?? "")));
    }
    if ($method === "POST" && $action === "icon_upload") {
        $file = $_FILES["file"] ?? null;
        if (
            !$file ||
            $file["error"] !== UPLOAD_ERR_OK ||
            !is_uploaded_file($file["tmp_name"])
        ) {
            throw new ApiError(422, "画像をアップロードできませんでした。");
        }
        respond(
            $icons->save(
                (string) file_get_contents($file["tmp_name"]),
                "Administrator upload",
            ),
        );
    }
    throw new ApiError(404, "APIが見つかりません。");
} catch (ApiError $e) {
    respond(["error" => $e->getMessage()], $e->status);
} catch (\Throwable $e) {
    error_log("Launchpad admin: " . $e->getMessage());
    respond(
        ["error" => "処理に失敗しました。サーバーログを確認してください。"],
        500,
    );
}
