declare module "@tauri-apps/api/core" {
  export function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
}

declare module "@tauri-apps/plugin-opener" {
  export function openUrl(url: string): Promise<void>;
}
