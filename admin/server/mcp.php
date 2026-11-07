<?php
declare(strict_types=1);
namespace Launchpad;
ini_set("display_errors", "0");
require __DIR__ . "/Contract.php";
require __DIR__ . "/CatalogStore.php";
require __DIR__ . "/TokenStore.php";
require __DIR__ . "/McpServer.php";
require __DIR__ . "/Icons.php";
require __DIR__ . "/Inbox.php";
require __DIR__ . "/Collector.php";
header("Content-Type: application/json");
header("Cache-Control: no-store");
header("X-Content-Type-Options: nosniff");
function reject(int $status, string $message, int $code = -32600): never
{
    http_response_code($status);
    echo json_encode([
        "jsonrpc" => "2.0",
        "id" => null,
        "error" => ["code" => $code, "message" => $message],
    ]);
    exit();
}
try {
    $path = getenv("LAUNCHPAD_ADMIN_CONFIG") ?: __DIR__ . "/config.php";
    if (!is_file($path)) {
        reject(503, "Server not configured");
    }
    $config = require $path;
    $secure = ($_SERVER["HTTPS"] ?? "") === "on";
    if (
        !$secure &&
        !(
            ($config["allow_local_http"] ?? false) &&
            in_array($_SERVER["REMOTE_ADDR"] ?? "", ["127.0.0.1", "::1"], true)
        )
    ) {
        reject(403, "HTTPS required");
    }
    // Native clients omit Origin. Browser origins must be explicitly configured.
    if (
        isset($_SERVER["HTTP_ORIGIN"]) &&
        !in_array(
            $_SERVER["HTTP_ORIGIN"],
            $config["mcp_allowed_origins"] ?? [],
            true,
        )
    ) {
        reject(403, "Origin not allowed");
    }
    $store = new CatalogStore(
        $config["storage"],
        $config["public_catalog"],
        $config["seed"],
    );
    $tokens = new TokenStore($config["storage"] . "/mcp-token.json", $store);
    $authorization =
        $_SERVER["HTTP_AUTHORIZATION"] ??
        ($_SERVER["REDIRECT_HTTP_AUTHORIZATION"] ?? "");
    if (
        !str_starts_with($authorization, "Bearer ") ||
        !$tokens->verify(substr($authorization, 7))
    ) {
        header('WWW-Authenticate: Bearer realm="launchpad-console"');
        reject(401, "Invalid or expired bearer token");
    }
    if ($_SERVER["REQUEST_METHOD"] !== "POST") {
        header("Allow: POST");
        reject(405, "Only POST is supported");
    }
    if (!str_contains($_SERVER["CONTENT_TYPE"] ?? "", "application/json")) {
        reject(415, "Expected application/json");
    }
    $accept = $_SERVER["HTTP_ACCEPT"] ?? "";
    if (
        !str_contains($accept, "application/json") ||
        !str_contains($accept, "text/event-stream")
    ) {
        reject(
            406,
            "Accept must include application/json and text/event-stream",
        );
    }
    $version = $_SERVER["HTTP_MCP_PROTOCOL_VERSION"] ?? null;
    if ($version !== null && !in_array($version, McpServer::VERSIONS, true)) {
        reject(400, "Unsupported protocol version");
    }
    $raw = file_get_contents(
        "php://input",
        false,
        null,
        0,
        Contract::MAX_BYTES + 1,
    );
    if (strlen($raw) > Contract::MAX_BYTES) {
        reject(413, "Request too large");
    }
    try {
        $request = json_decode($raw, true, 64, JSON_THROW_ON_ERROR);
    } catch (\JsonException) {
        reject(400, "Parse error", -32700);
    }
    if (!is_array($request) || array_is_list($request)) {
        reject(400, "Expected a single JSON-RPC message");
    }
    if (!array_key_exists("id", $request)) {
        if (
            ($request["jsonrpc"] ?? "") !== "2.0" ||
            !is_string($request["method"] ?? null) ||
            !str_starts_with($request["method"], "notifications/")
        ) {
            reject(400, "Invalid notification");
        }
        http_response_code(202);
        exit();
    }
    $icons = new Icons(
        $config["public_catalog"] . "/icons",
        $config["storage"],
    );
    $inbox = new Inbox($config["storage"], $store, $icons);
    $collector = new Collector($config["storage"], $store, $inbox, $icons);
    echo json_encode(
        (new McpServer($store, $inbox, $collector))->handle($request),
        JSON_UNESCAPED_UNICODE | JSON_THROW_ON_ERROR,
    );
} catch (\Throwable $e) {
    error_log("Launchpad MCP: " . $e->getMessage());
    reject(500, "Internal error", -32603);
}
