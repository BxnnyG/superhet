import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { databaseStatus, fakeAdsbApi } from "../adsb/testing";
import { AircraftDbSection } from "./AircraftDbSection";

beforeEach(() => vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "setTimeout"] }));
afterEach(() => vi.useRealTimers());

const flush = () => act(() => vi.advanceTimersByTimeAsync(0));

describe("AircraftDbSection", () => {
  it("shows source, licence and content of the imported database", async () => {
    const { api } = fakeAdsbApi();
    render(<AircraftDbSection api={api} />);
    await flush();
    const info = screen.getByTestId("aircraft-db-info");
    expect(info).toHaveTextContent("Mictronics aircraft database");
    expect(within(info).getByRole("link", { name: "ODC-By 1.0" })).toHaveAttribute(
      "href",
      "https://opendatacommons.org/licenses/by/1-0/",
    );
    expect(info).toHaveTextContent("455,928 aircraft");
    expect(info).toHaveTextContent("downloaded");
    // The offer says what a download fetches before anything is fetched.
    expect(
      screen.getByText(/Nothing is asked from a third party per aircraft/),
    ).toBeInTheDocument();
  });

  it("starts a download and polls while the import runs", async () => {
    const { api, state } = fakeAdsbApi();
    state.database = databaseStatus({ database: null });
    render(<AircraftDbSection api={api} />);
    await flush();
    expect(screen.getByTestId("aircraft-db-empty")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import from file" })).toBeEnabled();

    state.database = databaseStatus({
      database: null,
      job: { state: "downloading", message: "", finished_ms: null },
    });
    fireEvent.click(screen.getByRole("button", { name: "Download aircraft database" }));
    await flush();
    expect(api.updateDatabase).toHaveBeenCalledWith("download");
    expect(screen.getByText("Downloading the database…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Download aircraft database" })).toBeDisabled();

    const calls = api.database.mock.calls.length;
    state.database = databaseStatus({
      job: { state: "done", message: "3 aircraft", finished_ms: 1 },
    });
    await act(() => vi.advanceTimersByTimeAsync(3100));
    expect(api.database.mock.calls.length).toBeGreaterThan(calls);
    expect(screen.getByTestId("aircraft-db-info")).toBeInTheDocument();
    expect(screen.getByText(/Last import: 3 aircraft/)).toBeInTheDocument();
  });

  it("disables the file import when there is nothing to import", async () => {
    const { api, state } = fakeAdsbApi();
    state.database = databaseStatus({
      database: null,
      offer: { ...databaseStatus().offer, file: null, archive: false, download_url: "x" },
    });
    render(<AircraftDbSection api={api} />);
    await flush();
    expect(screen.getByRole("button", { name: "Import from file" })).toBeDisabled();
  });

  it("reports a failed import and removes the database after a confirmation", async () => {
    const { api, state } = fakeAdsbApi();
    state.database = databaseStatus({
      job: { state: "failed", message: "download failed: the server answered 404", finished_ms: 1 },
    });
    render(<AircraftDbSection api={api} />);
    await flush();
    expect(screen.getByText(/Last import failed: download failed/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(api.removeDatabase).not.toHaveBeenCalled();
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    await flush();
    expect(api.removeDatabase).toHaveBeenCalledTimes(1);
  });

  it("shows an error from the server", async () => {
    const { api } = fakeAdsbApi();
    api.updateDatabase.mockRejectedValueOnce(new Error("an import is already running"));
    render(<AircraftDbSection api={api} />);
    await flush();
    fireEvent.click(screen.getByRole("button", { name: "Download / update" }));
    await flush();
    expect(screen.getByText(/already running/)).toBeInTheDocument();
  });
});
