// How the page reaches the engine. In the desktop window that is the Rust host
// (Tauri's invoke), which holds the engine token. In a plain browser during
// development there is no host, so the dev server does the same job at
// /__host/<command> (see vite.config.ts). A production build always uses the host.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";

const inDesktop = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export async function invoke<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
  if (!import.meta.env.DEV || inDesktop()) return tauriInvoke<T>(cmd, args);
  const res = await fetch(`/__host/${cmd}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(args),
  });
  const body = await res.json().catch(() => ({}));
  // The desktop host rejects with a plain string, so do the same here.
  if (!res.ok) throw (body as { error?: string }).error ?? "engine not reachable";
  return body as T;
}
