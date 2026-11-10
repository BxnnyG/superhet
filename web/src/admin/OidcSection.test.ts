import { describe, expect, it } from "vitest";
import { DEFAULT_OIDC_SETTINGS, oidcSettingsBody, parseOidcView, type OidcSettings } from "./api";
import { oidcProblem, parseList } from "./OidcSection";

const ok: OidcSettings = {
  ...DEFAULT_OIDC_SETTINGS,
  enabled: true,
  issuer: "https://idp.example.org",
  clientId: "superhet",
  publicUrl: "https://sdr.example.org",
  adminGroups: ["sdr-admins"],
};

describe("oidcProblem", () => {
  it("accepts a complete configuration and a disabled one", () => {
    expect(oidcProblem(ok)).toBeNull();
    expect(oidcProblem({ ...DEFAULT_OIDC_SETTINGS })).toBeNull();
  });

  it("refuses plain http except for localhost", () => {
    expect(oidcProblem({ ...ok, issuer: "http://idp.example.org" })).toMatch(/https/);
    expect(oidcProblem({ ...ok, issuer: "http://localhost:8080" })).toBeNull();
  });

  it("refuses credentials in URLs and a missing client id", () => {
    expect(oidcProblem({ ...ok, publicUrl: "https://a:b@sdr.example.org" })).toMatch(/credentials/);
    expect(oidcProblem({ ...ok, clientId: " " })).toMatch(/client ID/);
  });

  it("needs a group mapping or a default role", () => {
    expect(oidcProblem({ ...ok, adminGroups: [] })).toMatch(/nobody/);
    expect(oidcProblem({ ...ok, adminGroups: [], defaultRole: "listener" })).toBeNull();
  });
});

describe("settings documents", () => {
  it("splits lists", () => {
    expect(parseList(" a, b ,,c ")).toEqual(["a", "b", "c"]);
  });

  it("round-trips the settings through the wire format without a secret", () => {
    const body = oidcSettingsBody(ok);
    expect(JSON.stringify(body)).not.toMatch(/secret/);
    const view = parseOidcView({
      oidc: body,
      source: "env",
      redirect_url: "https://sdr.example.org/api/v1/auth/oidc/callback",
      client_secret_set: true,
    });
    expect(view.settings).toEqual(ok);
    expect(view.source).toBe("env");
    expect(view.clientSecretSet).toBe(true);
  });

  it("carries the logout switches and the URLs to register", () => {
    const view = parseOidcView({
      oidc: { ...oidcSettingsBody(ok), rp_logout: false, backchannel_logout: false },
      post_logout_url: "https://sdr.example.org/",
      backchannel_url: "https://sdr.example.org/api/v1/auth/oidc/backchannel-logout",
    });
    expect(view.settings.rpLogout).toBe(false);
    expect(view.settings.backchannelLogout).toBe(false);
    expect(view.postLogoutUrl).toBe("https://sdr.example.org/");
    expect(view.backchannelUrl).toMatch(/backchannel-logout$/);
    const body = oidcSettingsBody(view.settings);
    expect(body).toMatchObject({ rp_logout: false, backchannel_logout: false });
    // Older servers do not send them: both default on.
    const old = parseOidcView({ oidc: { issuer: "https://idp.example" } });
    expect(old.settings.rpLogout && old.settings.backchannelLogout).toBe(true);
  });
});
