import { describe, expect, it } from "vitest";
import { issuerHint, publicUrlMismatch } from "./oidcHints";

describe("issuerHint", () => {
  it("suggests the issuer for endpoint URLs of the common providers", () => {
    const cases: Array<[string, string]> = [
      ["https://auth.example/oauth/v2/authorize", "https://auth.example"],
      ["https://auth.example/authorize", "https://auth.example"],
      ["https://auth.example/.well-known/openid-configuration", "https://auth.example"],
      [
        "https://auth.example/realms/home/protocol/openid-connect/auth",
        "https://auth.example/realms/home",
      ],
      ["https://auth.example:8443/api/oidc/token", "https://auth.example:8443"],
    ];
    for (const [given, want] of cases) {
      expect(issuerHint(given)?.suggested, given).toBe(want);
    }
  });

  it("explains without guessing where the issuer cannot be derived", () => {
    const hint = issuerHint("https://auth.example/application/o/authorize/");
    expect(hint?.suggested).toBeNull();
    expect(hint?.message).toMatch(/not its issuer/);
  });

  it("leaves real issuers and non-URLs alone", () => {
    for (const issuer of [
      "https://auth.example",
      "https://auth.example/realms/home",
      "https://auth.example/application/o/superhet/",
      "",
      "not a url",
    ]) {
      expect(issuerHint(issuer), issuer).toBeNull();
    }
  });
});

describe("publicUrlMismatch", () => {
  it("is quiet when the public URL is where the page is open", () => {
    expect(publicUrlMismatch("https://sdr.example.org/", "https://sdr.example.org")).toBeNull();
    expect(publicUrlMismatch("", "https://sdr.example.org")).toBeNull();
  });

  it("warns about another host, scheme or port", () => {
    for (const url of [
      "https://other.example.org",
      "http://sdr.example.org",
      "https://sdr.example.org:8443",
    ]) {
      expect(publicUrlMismatch(url, "https://sdr.example.org"), url).toMatch(/not the address/);
    }
  });
});
