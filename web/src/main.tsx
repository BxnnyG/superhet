import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AuthGate } from "./auth/AuthGate";
import "./index.css";
import { Root } from "./Root";
import { applyTheme, loadTheme } from "./ui/theme";

const root = document.getElementById("root");
if (!root) {
  throw new Error("Root element #root not found");
}

applyTheme(loadTheme());

createRoot(root).render(
  <StrictMode>
    <AuthGate>
      <Root />
    </AuthGate>
  </StrictMode>,
);
