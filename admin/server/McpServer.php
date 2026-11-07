<?php
declare(strict_types=1);
namespace Launchpad;
final class McpServer
{
    public const VERSIONS = ["2025-11-25", "2025-06-18", "2025-03-26"];
    public function __construct(
        private CatalogStore $store,
        private ?Inbox $inbox = null,
        private ?Collector $collector = null,
    ) {}
    private function tools(): array
    {
        $string = ["type" => "string"];
        $revision = [
            "type" => "string",
            "description" =>
                "Exact revision from get_catalog; prevents overwriting concurrent edits.",
        ];
        $specs = [
            [
                "list_inbox",
                "Read automatically collected Windows/Mac registration candidates. Only unresolved entries need classification.",
                [],
                [],
                true,
            ],
            [
                "import_inventory",
                "Ingest up to 100 apps from the explicit administrator desktop export or researched exact identities. Generates deduplicated candidates, never publishes.",
                [
                    "inventory" => [
                        "type" => "object",
                        "description" =>
                            "schema_version:1, source:desktop, apps:[{name,platform,rule,suggested_folder_id?,category?,icon_png?}]. Use exact OS identities, not guesses.",
                    ],
                ],
                ["inventory"],
                false,
            ],
            [
                "accept_inbox",
                "Merge selected candidates into the draft. Supply OS-compatible folder IDs. Does not publish.",
                [
                    "selections" => [
                        "type" => "array",
                        "items" => [
                            "type" => "object",
                            "properties" => [
                                "id" => ["type" => "string"],
                                "folder_id" => ["type" => "string"],
                            ],
                            "required" => ["id", "folder_id"],
                            "additionalProperties" => false,
                        ],
                    ],
                    "revision" => $revision,
                ],
                ["selections", "revision"],
                false,
            ],
            [
                "collect_sources",
                "Fetch the saved public App Store search sources and generate registration candidates. May save high-confidence drafts if enabled; never publishes.",
                [],
                [],
                false,
            ],
            [
                "get_catalog",
                "Read the classification draft and revision. Contains Windows and Mac rules, folders and icon references.",
                [],
                [],
                true,
            ],
            [
                "save_catalog",
                "Validate and replace the draft only. Does not publish. Read get_catalog first and preserve unrelated entries.",
                [
                    "document" => [
                        "type" => "object",
                        "description" =>
                            "Complete schema_version 1 document from get_catalog, with requested edits.",
                    ],
                    "revision" => $revision,
                ],
                ["document", "revision"],
                false,
            ],
            [
                "list_releases",
                "Read the live version and release history.",
                [],
                [],
                true,
            ],
            [
                "publish_catalog",
                "Publish the saved draft as static JSON to all launcher clients. Only call when the owner explicitly requests publication.",
                ["revision" => $revision],
                ["revision"],
                false,
            ],
            [
                "restore_release",
                "Republish a past release as a new version; replaces the current draft. Only call when the owner explicitly requests rollback.",
                [
                    "revision" => $revision,
                    "version" => ["type" => "integer", "minimum" => 1],
                ],
                ["revision", "version"],
                false,
            ],
        ];
        return array_map(
            fn($s) => [
                "name" => $s[0],
                "description" => $s[1],
                "inputSchema" => [
                    "type" => "object",
                    "properties" => (object) $s[2],
                    "required" => $s[3],
                    "additionalProperties" => false,
                ],
                "annotations" => [
                    "readOnlyHint" => $s[4],
                    "destructiveHint" => !$s[4],
                    "idempotentHint" => $s[4],
                    "openWorldHint" => $s[0] === "collect_sources",
                ],
            ],
            $specs,
        );
    }
    public function handle(array $request): array
    {
        $id = $request["id"] ?? null;
        $error = fn($code, $message) => [
            "jsonrpc" => "2.0",
            "id" => $id,
            "error" => ["code" => $code, "message" => $message],
        ];
        if (
            ($request["jsonrpc"] ?? "") !== "2.0" ||
            !is_string($request["method"] ?? null) ||
            (!is_int($id) && !is_string($id))
        ) {
            return $error(-32600, "Invalid Request");
        }
        $params = $request["params"] ?? [];
        if (!is_array($params)) {
            return $error(-32602, "Invalid params");
        }
        $method = $request["method"];
        if ($method === "initialize") {
            $version = $params["protocolVersion"] ?? "";
            $result = [
                "protocolVersion" => in_array($version, self::VERSIONS, true)
                    ? $version
                    : self::VERSIONS[0],
                "capabilities" => ["tools" => (object) []],
                "serverInfo" => [
                    "name" => "launchpad-console",
                    "version" => "1.0.0",
                ],
                "instructions" =>
                    "Manage Windows and Mac classification. Save drafts separately from publication. Preserve existing rules unless asked to change them.",
            ];
        } elseif ($method === "ping") {
            $result = (object) [];
        } elseif ($method === "tools/list") {
            $result = ["tools" => $this->tools()];
        } elseif ($method === "tools/call") {
            $name = $params["name"] ?? "";
            $args = $params["arguments"] ?? [];
            $spec = null;
            foreach ($this->tools() as $tool) {
                if ($tool["name"] === $name) {
                    $spec = $tool;
                }
            }
            if (!$spec) {
                return $error(-32602, "Unknown tool");
            }
            if (!is_array($args)) {
                return $error(-32602, "Invalid arguments");
            }
            $schema = $spec["inputSchema"];
            if (
                array_diff($schema["required"], array_keys($args)) ||
                array_diff(
                    array_keys($args),
                    array_keys((array) $schema["properties"]),
                )
            ) {
                return $error(-32602, "Missing or unknown arguments");
            }
            foreach ((array) $schema["properties"] as $key => $field) {
                if (!array_key_exists($key, $args)) {
                    continue;
                }
                if (
                    match ($field["type"]) {
                        "string" => !is_string($args[$key]),
                        "integer" => !is_int($args[$key]) || $args[$key] < 1,
                        "object" => !is_array($args[$key]),
                        "array" => !is_array($args[$key]) ||
                            !array_is_list($args[$key]),
                        default => true,
                    }
                ) {
                    return $error(-32602, "Invalid argument: " . $key);
                }
            }
            try {
                $data = match ($name) {
                    "list_inbox" => [
                        "candidates" => array_values($this->inbox->list()),
                    ],
                    "import_inventory" => $this->inbox->ingest(
                        $args["inventory"],
                    ),
                    "accept_inbox" => $this->inbox->accept(
                        $args["selections"],
                        $args["revision"],
                    ),
                    "collect_sources" => $this->collector->run(),
                    "get_catalog" => $this->store->draft(),
                    "list_releases" => $this->store->status(),
                    "save_catalog" => $this->store->save(
                        $args["document"],
                        $args["revision"],
                    ),
                    "publish_catalog" => $this->store->publish(
                        $args["revision"],
                    ),
                    "restore_release" => $this->store->publish(
                        $args["revision"],
                        $args["version"],
                    ),
                };
                $result = [
                    "content" => [
                        ["type" => "text", "text" => Contract::encode($data)],
                    ],
                    "structuredContent" => $data,
                    "isError" => false,
                ];
            } catch (ApiError $e) {
                $result = [
                    "content" => [
                        ["type" => "text", "text" => $e->getMessage()],
                    ],
                    "isError" => true,
                ];
            }
        } else {
            return $error(-32601, "Method not found");
        }
        return ["jsonrpc" => "2.0", "id" => $id, "result" => $result];
    }
}
