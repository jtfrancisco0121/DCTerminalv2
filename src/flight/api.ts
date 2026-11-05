/** The only invoke wrappers of the Session Inspector. */
import { invoke } from "@tauri-apps/api/core";
import type {
  CompareOut,
  ContentOut,
  EvalResults,
  ExportEncoding,
  ExportPolicy,
  ExportPreview,
  ExportSent,
  SearchFilter,
  SearchOut,
  SessionView,
  SourceRef,
  TraceTarget,
} from "./types";

/** `focus`: an event id; with no cursor the first page is the one holding it. */
export type SessionRequest = TraceTarget & { agent?: string; cursor?: string; pageSize?: number; focus?: string };

export const flightSession = (request: SessionRequest) => invoke<SessionView>("flight_session", { request });

export const flightContent = (target: TraceTarget, ref: SourceRef) =>
  invoke<ContentOut>("flight_content", { request: { ...target, ref } });

/** The structured trace search over the local index: values only, never SQL text. */
export const flightSearch = (filter: SearchFilter) => invoke<SearchOut>("flight_search", { filter });

/** Two runs side by side: the rows in a fixed order and the first difference. Read-only; never indexes. */
export const flightCompare = (a: string, b: string) => invoke<CompareOut>("flight_compare", { request: { a, b } });

/** The runs of one eval group with their measures and min / median / max over the finished ones (read-only). */
export const flightEvalResults = (group: string) => invoke<EvalResults>("flight_eval_results", { group });

/** What one finished run used of the account's five-hour window (an estimate in percent points); null when not known. */
export const flightRunShare = (runDir: string) => invoke<number | null>("flight_run_share", { runDir });

/** The Export Preview of a Trace: counts and the fields that would leave. The host keeps the previewed bytes. */
export const flightExportPreview = (
  target: TraceTarget,
  options: { policy: ExportPolicy; encoding: ExportEncoding; confirmFull: boolean; endpoint?: string },
) => invoke<ExportPreview>("flight_export_preview", { request: { ...target, ...options } });

/** Posts exactly the previewed bytes (`digest` names them) to `endpoint`. */
export const flightExportSend = (digest: string, endpoint: string) =>
  invoke<ExportSent>("flight_export_send", { request: { digest, endpoint } });

/** Writes exactly the previewed bytes to the picked path. */
export const flightExportSave = (digest: string, path: string) =>
  invoke<void>("flight_export_save", { request: { digest, path } });
