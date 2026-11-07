<?php
declare(strict_types=1);
return [
    // Generate with: php -r 'echo password_hash(trim(fgets(STDIN)), PASSWORD_DEFAULT), PHP_EOL;'
    "password_hash" => "REPLACE_WITH_PASSWORD_HASH",
    "storage" => "/home/ACCOUNT/DOMAIN/launchpad-storage",
    "public_catalog" => "/home/ACCOUNT/DOMAIN/public_html/catalog",
    "seed" => "/home/ACCOUNT/DOMAIN/server/data/catalog-v1.json",
    "allow_local_http" => false,
    // Native MCP clients omit Origin. Add exact browser origins only when needed.
    "mcp_allowed_origins" => [],
];
