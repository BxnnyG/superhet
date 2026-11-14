import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  createBackupApi,
  parseOverview,
  parseReport,
  type BackupApi,
  type ImportReport,
} from "./backupApi";
import { BackupSection } from "./BackupSection";

const overviewJson = {
  areas: [
    { id: "profiles", title: "Profiles", description: "p", optional: false, credentials: "" },
    { id: "users", title: "Accounts", description: "u", optional: false, credentials: "hashes" },
    { id: "audit", title: "Audit log", description: "a", optional: true, credentials: "" },
  ],
  max_upload_bytes: 1 << 30,
  min_passphrase_length: 8,
  schedule: null,
};

const report: ImportReport = {
  dryRun: true,
  createdAt: "2026-11-13T00:00:00Z",
  createdBy: "admin",
  appVersion: "0.1.0",
  schemaVersion: 18,
  currentSchemaVersion: 18,
  includesCredentials: false,
  areas: ["profiles"],
  tables: [
    {
      table: "profiles",
      area: "profiles",
      inArchive: 2,
      existing: 1,
      deleted: 1,
      inserted: 2,
      updated: 0,
      skipped: 0,
    },
  ],
  filesInArchive: 0,
  filesRestored: 0,
  warnings: ["something to know"],
  conflicts: ['your own account "admin" differs in role: kept as it is'],
};

function fakeApi(): BackupApi & { calls: unknown[] } {
  const calls: unknown[] = [];
  return {
    calls,
    overview: () => Promise.resolve(parseOverview(overviewJson)),
    exportArchive: (req) => {
      calls.push(req);
      return Promise.resolve({ blob: new Blob(["x"]), filename: "b.zip" });
    },
    importArchive: (_file, req) => {
      calls.push(req);
      return Promise.resolve({ ...report, dryRun: req.dryRun });
    },
  };
}

describe("BackupSection", () => {
  it("exports the default areas, without credentials unless ticked", async () => {
    URL.createObjectURL = vi.fn(() => "blob:x");
    URL.revokeObjectURL = vi.fn();
    const api = fakeApi();
    render(<BackupSection api={api} />);
    await screen.findByText("Create a backup");
    expect(screen.getByLabelText(/Profiles/)).toBeChecked();
    expect(screen.getByLabelText(/Audit log/)).not.toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Create and download" }));
    await waitFor(() => expect(api.calls).toHaveLength(1));
    expect(api.calls[0]).toEqual({
      areas: ["profiles", "users"],
      includeCredentials: false,
      passphrase: "",
    });
  });

  it("refuses a short passphrase", async () => {
    render(<BackupSection api={fakeApi()} />);
    await screen.findByText("Create a backup");
    fireEvent.change(screen.getAllByLabelText(/Passphrase/)[0] as HTMLElement, {
      target: { value: "short" },
    });
    expect(screen.getByRole("button", { name: "Create and download" })).toBeDisabled();
    expect(screen.getByText(/at least 8 characters/)).toBeInTheDocument();
  });

  it("checks first, shows conflicts, and restores only after confirmation", async () => {
    const api = fakeApi();
    render(<BackupSection api={api} />);
    await screen.findByText("Restore a backup");
    const restore = screen.getByRole("button", { name: "Restore" });
    expect(restore).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Backup file"), {
      target: { files: [new File(["zip"], "b.zip")] },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check backup" }));
    });
    const view = await screen.findByTestId("backup-report");
    expect(view).toHaveTextContent("your own account");
    expect(view).toHaveTextContent("something to know");
    expect(api.calls.at(-1)).toMatchObject({ dryRun: true, overwriteSelf: false });
    fireEvent.click(screen.getByRole("button", { name: "Restore" }));
    expect(api.calls).toHaveLength(1);
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Restore" }));
    await waitFor(() => expect(api.calls).toHaveLength(2));
    expect(api.calls[1]).toMatchObject({ dryRun: false });
  });
});

describe("backup api", () => {
  it("sends the passphrase percent-encoded in a header and parses the report", async () => {
    const fetchImpl = vi.fn(() =>
      Promise.resolve(
        new Response(
          JSON.stringify({ report: { dry_run: true, backup: { created_by: "x" }, tables: [] } }),
          { status: 200 },
        ),
      ),
    );
    const api = createBackupApi(fetchImpl);
    const out = await api.importArchive(new Blob(["z"]), {
      dryRun: true,
      overwriteSelf: true,
      areas: ["profiles"],
      passphrase: "pässwörd 1",
    });
    expect(out.createdBy).toBe("x");
    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/v1/admin/backup/import?dry_run=true&overwrite_self=true&areas=profiles");
    expect((init.headers as Record<string, string>)["X-Backup-Passphrase"]).toBe(
      encodeURIComponent("pässwörd 1"),
    );
  });

  it("turns an error answer into the server's message", async () => {
    const fetchImpl = vi.fn(() =>
      Promise.resolve(
        new Response(JSON.stringify({ error: "backup_newer", message: "update Superhet first" }), {
          status: 409,
        }),
      ),
    );
    await expect(
      createBackupApi(fetchImpl).importArchive(new Blob(["z"]), {
        dryRun: true,
        overwriteSelf: false,
        areas: [],
        passphrase: "",
      }),
    ).rejects.toThrow("update Superhet first");
    expect(parseReport(undefined).tables).toEqual([]);
  });
});
