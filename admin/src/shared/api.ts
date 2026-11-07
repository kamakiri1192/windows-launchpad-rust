let csrf = "";
export function setCsrf(value: string) {
  csrf = value;
}
export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}
export async function api<T>(
  action: string,
  data?: unknown,
  query?: Record<string, string>,
): Promise<T> {
  const url = new URL("api/index.php", document.baseURI);
  url.searchParams.set("action", action);
  for (const [key, value] of Object.entries(query ?? {}))
    url.searchParams.set(key, value);
  const form = data instanceof FormData;
  const response = await fetch(url, {
    method: data === undefined ? "GET" : "POST",
    credentials: "same-origin",
    headers:
      data === undefined
        ? {}
        : {
            "X-CSRF-Token": csrf,
            ...(!form ? { "Content-Type": "application/json" } : {}),
          },
    body: data === undefined ? undefined : form ? data : JSON.stringify(data),
  });
  const result = await response
    .json()
    .catch(() => ({ error: "サーバーの応答を読み込めません。" }));
  if (!response.ok) {
    if (response.status === 401 && action !== "login")
      window.dispatchEvent(new Event("session-expired"));
    throw new ApiError(response.status, result.error ?? "通信に失敗しました。");
  }
  return result as T;
}
export function iconUrl(path: string) {
  return new URL(`catalog/${path}`, document.baseURI).href;
}
export function message(error: unknown) {
  return error instanceof Error ? error.message : "処理に失敗しました。";
}
