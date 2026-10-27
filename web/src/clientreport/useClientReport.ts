import { useEffect, useRef } from "react";
import { runClientReport, type ClientReportRequest, type ReportDeps } from "./collect";

/**
 * Report this browser's details once per page load and signed-in identity, when the server asks
 * (`request` non-null). A different account in the same page (sign out, sign in) reports again;
 * a status refresh does not.
 */
export function useClientReport(
  request: ClientReportRequest | null,
  identity: string | null,
  deps?: ReportDeps,
): void {
  const done = useRef<string | null>(null);
  const service = request?.publicIpService ?? null;
  const asked = request !== null;
  useEffect(() => {
    if (!asked || identity === null) {
      done.current = null;
      return;
    }
    const key = `${identity}|${service ?? ""}`;
    if (done.current === key) return;
    done.current = key;
    void runClientReport({ publicIpService: service }, deps);
    // `deps` only differs in tests and is read once per key.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [asked, identity, service]);
}
