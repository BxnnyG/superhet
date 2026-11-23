import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HttpError } from "../http";
import { toasts } from "../ui";
import { DEFAULT_OIDC_SETTINGS, type AdminApi, type OidcTestResult, type OidcView } from "./api";
import { OidcSection } from "./OidcSection";

const view = (over: Partial<OidcView> = {}): OidcView => ({
  settings: { ...DEFAULT_OIDC_SETTINGS, publicUrl: "https://sdr.example.org" },
  source: "database",
  redirectUrl: "https://sdr.example.org/api/v1/auth/oidc/callback",
  postLogoutUrl: null,
  backchannelUrl: null,
  clientSecretSet: false,
  oidcAdminExists: false,
  localLoginActive: true,
  keepLocalLogin: false,
  lastError: null,
  discovery: null,
  ...over,
});

const failed = (over: Partial<OidcTestResult> = {}): OidcTestResult => ({
  ok: false,
  issuer: null,
  error: "discovery failed",
  endSession: false,
  status: 404,
  url: "https://auth.example/oauth/v2/authorize/.well-known/openid-configuration",
  hint: null,
  suggestedIssuer: null,
  ...over,
});

function api(over: Partial<AdminApi> = {}): AdminApi {
  return {
    getOidc: vi.fn().mockResolvedValue(view()),
    putOidc: vi.fn(),
    testOidc: vi.fn(),
    ...over,
  } as unknown as AdminApi;
}

function issuerField(): HTMLInputElement {
  return screen.getByLabelText("Issuer URL") as HTMLInputElement;
}

beforeEach(() => toasts.clear());
afterEach(() => toasts.clear());

describe("OidcSection issuer help", () => {
  it("names an endpoint URL as the issuer and applies the suggestion only on click", async () => {
    render(<OidcSection api={api()} />);
    await screen.findByLabelText("Issuer URL");
    fireEvent.change(issuerField(), {
      target: { value: "https://auth.example/oauth/v2/authorize" },
    });
    expect(screen.getByText(/authorization endpoint/)).toBeTruthy();
    // The typed value stays until "Use this issuer" is pressed.
    expect(issuerField().value).toBe("https://auth.example/oauth/v2/authorize");
    fireEvent.click(screen.getByRole("button", { name: "Use this issuer" }));
    expect(issuerField().value).toBe("https://auth.example");
    expect(screen.queryByRole("button", { name: "Use this issuer" })).toBeNull();
  });

  it("offers the issuer the server confirmed after a failed save and keeps the entry as typed", async () => {
    const putOidc = vi.fn().mockResolvedValue(
      view({
        settings: {
          ...DEFAULT_OIDC_SETTINGS,
          enabled: true,
          issuer: "https://auth.example/authorize",
          clientId: "superhet",
          publicUrl: "https://sdr.example.org",
          defaultRole: "listener",
        },
        discovery: failed({ suggestedIssuer: "https://auth.example" }),
      }),
    );
    const enabled = view({
      settings: {
        ...DEFAULT_OIDC_SETTINGS,
        enabled: true,
        issuer: "https://auth.example",
        clientId: "superhet",
        publicUrl: "https://sdr.example.org",
        defaultRole: "listener",
      },
    });
    render(<OidcSection api={api({ putOidc, getOidc: vi.fn().mockResolvedValue(enabled) })} />);
    await screen.findByLabelText("Issuer URL");
    fireEvent.change(issuerField(), { target: { value: "https://auth.example/authorize" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await screen.findByText(/Saved, but the provider could not be used/);
    expect(screen.getByText(/HTTP 404 from https:\/\/auth.example/)).toBeTruthy();
    expect(issuerField().value).toBe("https://auth.example/authorize");
    fireEvent.click(screen.getAllByRole("button", { name: "Use this issuer" })[0] as HTMLElement);
    expect(issuerField().value).toBe("https://auth.example");
  });

  it("lets the admin keep an issuer the server refused", async () => {
    const putOidc = vi
      .fn()
      .mockRejectedValueOnce(
        new HttpError(400, "issuer_is_endpoint", "This looks like an endpoint."),
      )
      .mockResolvedValueOnce(view());
    render(<OidcSection api={api({ putOidc })} />);
    await screen.findByLabelText("Issuer URL");
    fireEvent.change(issuerField(), { target: { value: "https://auth.example/realms/token" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    const keep = await screen.findByRole("button", { name: "Save as entered" });
    fireEvent.click(keep);
    await waitFor(() => expect(putOidc).toHaveBeenCalledTimes(2));
    expect(putOidc.mock.calls[1]?.[1]).toEqual({ keepIssuer: true });
  });

  it("shows the last sign-in error to the admin", async () => {
    const lastError = {
      at: 1_700_000_000,
      issuer: "https://auth.example/authorize",
      url: "https://auth.example/authorize/.well-known/openid-configuration",
      status: 404,
      message: "Server returned invalid response",
    };
    render(<OidcSection api={api({ getOidc: vi.fn().mockResolvedValue(view({ lastError })) })} />);
    await screen.findByText(/last sign-in attempt could not reach the provider/);
    expect(screen.getByText(/HTTP 404 from https:\/\/auth.example\/authorize\//)).toBeTruthy();
  });

  it("shows the callback URL and warns when the public URL is not this page's address", async () => {
    render(<OidcSection api={api()} />);
    await screen.findByText(/Callback URL \(redirect URI\)/);
    expect(screen.getByText("https://sdr.example.org/api/v1/auth/oidc/callback")).toBeTruthy();
    // jsdom serves the page from http://localhost:3000, not from the public URL.
    expect(screen.getByText(/is not the address this page is open at/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Public URL of this instance"), {
      target: { value: window.location.origin },
    });
    expect(screen.queryByText(/is not the address this page is open at/)).toBeNull();
  });

  it("offers the suggestion a connection test found", async () => {
    const testOidc = vi.fn().mockResolvedValue(failed({ suggestedIssuer: "https://auth.example" }));
    render(
      <OidcSection
        api={api({
          testOidc,
          getOidc: vi.fn().mockResolvedValue(
            view({
              settings: { ...DEFAULT_OIDC_SETTINGS, issuer: "https://auth.example/x/token" },
            }),
          ),
        })}
      />,
    );
    await screen.findByLabelText("Issuer URL");
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));
    await screen.findByText(/different issuer on the same host/);
    fireEvent.click(
      screen.getAllByRole("button", { name: "Use this issuer" }).at(-1) as HTMLElement,
    );
    expect(issuerField().value).toBe("https://auth.example");
  });
});
