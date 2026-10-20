import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { App } from "./App";

it("renders the pre-alpha heading", () => {
  render(<App />);
  expect(screen.getByRole("heading", { name: /superhet – pre-alpha/i })).toBeInTheDocument();
});
