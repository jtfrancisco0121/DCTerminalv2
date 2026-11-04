/** The only invoke wrappers of the Session Inspector. */
import { invoke } from "@tauri-apps/api/core";
import type { CompareOut, ContentOut, SearchFilter, SearchOut, SessionView, SourceRef, TraceTarget } from "./types";

/** `focus`: an event id; with no cursor the first page is the one holding it. */
export type SessionRequest = TraceTarget & { agent?: string; cursor?: string; pageSize?: number; focus?: string };

export const flightSession = (request: SessionRequest) => invoke<SessionView>("flight_session", { request });

export const flightContent = (target: TraceTarget, ref: SourceRef) =>
  invoke<ContentOut>("flight_content", { request: { ...target, ref } });

/** The structured trace search over the local index: values only, never SQL text. */
export const flightSearch = (filter: SearchFilter) => invoke<SearchOut>("flight_search", { filter });

/** Two runs side by side: the rows in a fixed order and the first difference. Read-only; never indexes. */
export const flightCompare = (a: string, b: string) => invoke<CompareOut>("flight_compare", { request: { a, b } });
