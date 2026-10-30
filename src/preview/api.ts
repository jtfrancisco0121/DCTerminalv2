import { invoke } from "@tauri-apps/api/core";

export type Preview = { path: string; text: string; truncated: boolean; mtimeMs: number };
export type PreviewStat = { exists: boolean; mtimeMs: number; size: number };

export const previewFile = (path: string) => invoke<Preview>("preview_file", { path });
export const previewStat = (path: string) => invoke<PreviewStat>("preview_stat", { path });
