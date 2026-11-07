// Typed client for the plugin panel routes (docs/plugins-ui-panels.md).
//   GET /api/v1/plugin-panels                      -> { panels: [PanelInfo] }  (listener; guests with decoder access)
//   GET /api/v1/plugin-ui/{plugin_id}/{entry}      -> the panel page, loaded by the sandboxed frame

import { apiFetch } from "../auth/session";
import { ApiError, httpError, isRec, readJson } from "../spectrum/api";

export interface PanelInfo {
  pluginId: string;
  name: string;
  version: string;
  title: string;
  placement: "inspector";
  /** The page below the plugin's UI root (`index.html`). */
  entry: string;
  /** Plugin ids whose instances and events the panel may read. */
  reads: readonly string[];
  /** The panel may ask for a tune request (honoured for operators and admins only). */
  tune: boolean;
}

const ID = /^[a-z0-9][a-z0-9-]*(\.[a-z0-9][a-z0-9-]*){2,}$/;
const FILE = /^[A-Za-z0-9_-][A-Za-z0-9._-]*$/;

/** One panel from the API; null when it does not look right (it is then not shown). */
export function parsePanel(raw: unknown): PanelInfo | null {
  if (!isRec(raw)) return null;
  const { plugin_id, name, version, title, placement, entry, reads, tune } = raw;
  if (
    typeof plugin_id !== "string" ||
    !ID.test(plugin_id) ||
    typeof name !== "string" ||
    typeof version !== "string" ||
    typeof title !== "string" ||
    title === "" ||
    placement !== "inspector" ||
    typeof entry !== "string" ||
    !FILE.test(entry) ||
    !Array.isArray(reads)
  ) {
    return null;
  }
  const ids = reads.filter((r): r is string => typeof r === "string" && ID.test(r));
  return {
    pluginId: plugin_id,
    name,
    version,
    title,
    placement,
    entry,
    reads: ids.length > 0 ? ids : [plugin_id],
    tune: tune === true,
  };
}

/** URL of the panel page; it is public and sandboxed (no session cookie reaches it). */
export function panelUrl(p: Pick<PanelInfo, "pluginId" | "entry" | "version">): string {
  const enc = encodeURIComponent;
  return `/api/v1/plugin-ui/${enc(p.pluginId)}/${enc(p.entry)}?v=${enc(p.version)}`;
}

export interface PanelApi {
  list(signal?: AbortSignal): Promise<PanelInfo[]>;
}

export function createPanelApi(fetchImpl: typeof fetch = apiFetch): PanelApi {
  return {
    async list(signal) {
      let res: Response;
      try {
        res = await fetchImpl("/api/v1/plugin-panels", { signal });
      } catch (e) {
        if (e instanceof DOMException && e.name === "AbortError") throw e;
        throw new ApiError(e instanceof Error ? e.message : "network error");
      }
      const body = await readJson(res);
      if (!res.ok) throw httpError(res, body);
      if (!isRec(body) || !Array.isArray(body.panels)) throw new ApiError("invalid panel list");
      return body.panels.map(parsePanel).filter((p): p is PanelInfo => p !== null);
    },
  };
}
