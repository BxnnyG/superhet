import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { oidcErrorMessage, parseStatus } from "./api";
import { LoginPage } from "./LoginPage";
import { stubClient } from "./testing";

const SSO = { name: "Authentik", loginUrl: "/api/v1/auth/oidc/login" };

afterEach(() => {
  window.history.replaceState(null, "", "/");
});

describe("single sign-on on the login page", () => {
  it("offers the button next to the password form", () => {
    render(
      <LoginPage
        client={stubClient()}
        onSuccess={vi.fn()}
        options={{ sso: SSO, localLogin: true }}
      />,
    );
    const link = screen.getByRole("link", { name: "Sign in with Authentik" });
    expect(link).toHaveAttribute("href", "/api/v1/auth/oidc/login");
    expect(screen.getByLabelText("Password")).toBeInTheDocument();
  });

  it("hides the password form when local logins are switched off", () => {
    render(
      <LoginPage
        client={stubClient()}
        onSuccess={vi.fn()}
        options={{ sso: SSO, localLogin: false }}
      />,
    );
    expect(screen.getByRole("link", { name: "Sign in with Authentik" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Password")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Username")).not.toBeInTheDocument();
  });

  it("shows no button when SSO is off", () => {
    render(<LoginPage client={stubClient()} onSuccess={vi.fn()} />);
    expect(screen.queryByRole("link", { name: /Sign in with/ })).not.toBeInTheDocument();
    expect(screen.getByLabelText("Password")).toBeInTheDocument();
  });

  it("explains a failed SSO login once and cleans the address", async () => {
    window.history.replaceState(null, "", "/?oidc_error=no_account&x=1");
    render(
      <LoginPage
        client={stubClient()}
        onSuccess={vi.fn()}
        options={{ sso: SSO, localLogin: true }}
      />,
    );
    expect(await screen.findByText(/no account for you here yet/i)).toBeInTheDocument();
    expect(window.location.search).toBe("?x=1");
  });
});

describe("status parsing", () => {
  const base = { setup_required: false, authenticated: false };

  it("reads the SSO description and local login flag", () => {
    const st = parseStatus({
      ...base,
      oidc: { enabled: true, name: "Pocket ID", login_url: "/api/v1/auth/oidc/login" },
      local_login: false,
    });
    expect(st.oidc).toEqual({ name: "Pocket ID", loginUrl: "/api/v1/auth/oidc/login" });
    expect(st.localLogin).toBe(false);
  });

  it("defaults to password only for an older server", () => {
    const st = parseStatus(base);
    expect(st.oidc).toBeNull();
    expect(st.localLogin).toBe(true);
  });

  it("refuses login URLs that leave the site", () => {
    for (const login_url of ["https://evil.example/x", "//evil.example/x", "javascript:alert(1)"]) {
      expect(
        parseStatus({ ...base, oidc: { enabled: true, name: "X", login_url } }).oidc,
      ).toBeNull();
    }
    expect(
      parseStatus({ ...base, oidc: { enabled: false, name: "X", login_url: "/x" } }).oidc,
    ).toBeNull();
  });

  it("words every error code the server can send", () => {
    for (const code of [
      "no_account",
      "no_role",
      "disabled",
      "already_linked",
      "provider_denied",
      "invalid_state",
      "token_invalid",
      "rate_limited",
      "busy",
      "unavailable",
      "something_new",
    ]) {
      expect(oidcErrorMessage(code).length).toBeGreaterThan(10);
    }
  });
});
