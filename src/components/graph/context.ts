import type { ComputedRef, InjectionKey } from "vue";
import type { DateFormat } from "@/lib/bindings";

/** Per-graph values every row needs, provided once by `CommitGraphView` instead of prop-drilling. */
export interface GraphContext {
  /** Key of the session this graph shows, so ref pills can open the branch menu for it. */
  sessionKey: string;
  dateFormat: ComputedRef<DateFormat>;
  /** Local branch full name -> remote name of its upstream (`refs/heads/dev` -> `origin`). */
  upstreamRemotes: ComputedRef<Map<string, string>>;
}

export const GRAPH_CONTEXT: InjectionKey<GraphContext> = Symbol("graph-context");
