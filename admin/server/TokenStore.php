<?php
declare(strict_types=1);
namespace Launchpad;
final class TokenStore
{
    public function __construct(
        private string $path,
        private CatalogStore $store,
    ) {}
    private function read(): ?array
    {
        return is_file($this->path)
            ? json_decode(
                (string) file_get_contents($this->path),
                true,
                32,
                JSON_THROW_ON_ERROR,
            )
            : null;
    }
    public function status(): array
    {
        $data = $this->read();
        return [
            "active" => $data && $data["expires_at"] > time(),
            "expires_at" => $data["expires_at"] ?? null,
            "suffix" => $data["suffix"] ?? null,
        ];
    }
    public function rotate(): array
    {
        return $this->store->locked(function () {
            $token = "lp_" . bin2hex(random_bytes(32));
            $data = [
                "hash" => hash("sha256", $token),
                "expires_at" => time() + 90 * 86400,
                "suffix" => substr($token, -6),
            ];
            CatalogStore::atomic($this->path, Contract::encode($data));
            chmod($this->path, 0600);
            return ["token" => $token, ...$this->status()];
        });
    }
    public function revoke(): void
    {
        $this->store->locked(function () {
            if (is_file($this->path)) {
                unlink($this->path);
            }
        });
    }
    public function verify(string $token): bool
    {
        $data = $this->read();
        return $data &&
            $data["expires_at"] > time() &&
            hash_equals($data["hash"], hash("sha256", $token));
    }
}
