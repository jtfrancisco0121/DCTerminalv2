/** The only invoke wrappers of the Session Inspector. */
import { invoke } from "@tauri-apps/api/core";
import type { ContentOut, SessionView, SourceRef, TraceTarget } from "./types";

export type SessionRequest = TraceTarget & { agent?: string; cursor?: string; pageSize?: number };

export const flightSession = (request: SessionRequest) => invoke<SessionView>("flight_session", { request });

export const flightContent = (target: TraceTarget, ref: SourceRef) =>
  invoke<ContentOut>("flight_content", { request: { ...target, ref } });
