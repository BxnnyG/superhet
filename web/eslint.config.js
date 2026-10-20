import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "node_modules"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  reactHooks.configs.flat.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: { globals: globals.browser },
  },
  {
    // Design-system rule: raw <button>/<select> only inside the UI kit (docs/ui-design.md).
    files: ["src/**/*.tsx"],
    ignores: ["src/ui/**", "src/**/*.test.tsx"],
    rules: {
      "no-restricted-syntax": [
        "error",
        {
          selector: "JSXOpeningElement[name.name='button']",
          message: "Use Button/IconButton from src/ui instead of a raw <button>.",
        },
        {
          selector: "JSXOpeningElement[name.name='select']",
          message: "Use Select from src/ui instead of a raw <select>.",
        },
      ],
    },
  },
  {
    files: ["*.config.{js,ts}"],
    languageOptions: { globals: globals.node },
  },
);
