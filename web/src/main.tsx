import { StrictMode, Suspense, lazy } from "react";
import { createRoot } from "react-dom/client";
import { AuthGate } from "./auth/AuthGate";
import "./index.css";
import { Root } from "./Root";
import { sharePageOf } from "./share/paths";
import { applyTheme, loadTheme } from "./ui/theme";

// The public pages are a separate chunk: they need no login and share nothing with the console.
const ShareApp = lazy(() => import("./share/ShareApp").then((m) => ({ default: m.ShareApp })));

const root = document.getElementById("root");
if (!root) {
  throw new Error("Root element #root not found");
}

applyTheme(loadTheme());

// `/r/<token>` and `/embed/<token>` skip the login gate: the token is the whole credential.
const share = sharePageOf(window.location.pathname);

createRoot(root).render(
  <StrictMode>
    {share ? (
      <Suspense fallback={null}>
        <ShareApp token={share.token} embed={share.kind === "embed"} />
      </Suspense>
    ) : (
      <AuthGate>
        <Root />
      </AuthGate>
    )}
  </StrictMode>,
);
