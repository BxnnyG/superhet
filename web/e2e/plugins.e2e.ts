import { expect, test, type BrowserContext, type Page } from "@playwright/test";
import { readFile } from "node:fs/promises";
import path from "node:path";

// Browser smoke test of the plugin UI (ROADMAP P2): the admin sees the bundled plugins, installs
// the reference plugin from a path, starts it on the VFO of the file device, sees its state and
// its events in the Decoders tab (generic view, source filter, details, CSV export), and stops it
// in the admin console. The stack comes from scripts/e2e-stack.sh (plugin host with a private
// plugin directory; python3 runs plugins/example-echo). The first-party decoders are bundled but
// need multimon-ng / direwolf / rtl_433, which the golden tests of the adapter crate cover.

const SETUP_TOKEN = process.env.E2E_SETUP_TOKEN ?? "e2e-setup-token-0123456789";
const ADMIN = { username: "root", password: "admin passphrase for the smoke test" };
const ECHO_SRC =
  process.env.E2E_PLUGIN_SRC ?? path.resolve(process.cwd(), "..", "plugins", "example-echo");
const ECHO_ID = "org.superhet.example.echo";

test.describe.configure({ mode: "serial" });

// One signed-in browser context for the whole scenario (the login rate limit is per address).
let context: BrowserContext;
let page: Page;

test.beforeAll(async ({ browser }) => {
  context = await browser.newContext();
  page = await context.newPage();
});

test.afterAll(async () => {
  await context.close();
});

/** Sign in as the administrator, creating the account if this is the first test to run. */
async function signInAsAdmin() {
  await page.goto("/");
  const setup = page.getByRole("heading", { name: "Set up Superhet" });
  const signIn = page.getByRole("heading", { name: "Sign in" });
  const tuning = page.getByRole("region", { name: "Tuning" });
  await expect(setup.or(signIn).or(tuning)).toBeVisible();
  if (await setup.isVisible()) {
    await page.getByLabel("Username").fill(ADMIN.username);
    await page.getByLabel("Password", { exact: true }).fill(ADMIN.password);
    await page.getByLabel("Confirm password").fill(ADMIN.password);
    await page.getByLabel("Setup token").fill(SETUP_TOKEN);
    await page.getByRole("button", { name: "Create account" }).click();
  } else if (await signIn.isVisible()) {
    await page.getByLabel("Username").fill(ADMIN.username);
    await page.getByLabel("Password", { exact: true }).fill(ADMIN.password);
    await page.getByRole("button", { name: "Sign in" }).click();
  }
  await expect(tuning).toBeVisible();
}

async function openDecoders() {
  await page.getByRole("tab", { name: "Decoders" }).click();
  const panel = page.getByRole("tabpanel", { name: "Decoders" });
  await expect(panel).toBeVisible();
  return panel;
}

test("the admin sees the bundled plugins and installs the example plugin from a path", async () => {
  await signInAsAdmin();
  await page.goto("/admin/plugins");
  const list = page.getByRole("list", { name: "Installed plugins" });
  const pocsag = list.getByTestId("plugin-org.superhet.decoder.pocsag");
  await expect(pocsag).toContainText("POCSAG / FLEX pagers");
  await expect(pocsag).toContainText("Bundled");
  await expect(pocsag).toContainText("AGPL-3.0-only");
  await expect(pocsag).toContainText("Install notice");
  await expect(list.getByTestId(`plugin-${ECHO_ID}`)).toHaveCount(0);

  await page.getByRole("button", { name: "Install from path" }).click();
  const dialog = page.getByRole("dialog", { name: "Install a plugin" });
  await dialog.getByLabel("Directory with plugin.toml").fill(ECHO_SRC);
  await dialog.getByRole("button", { name: "Install", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Example echo installed" })).toBeVisible();
  await page.getByRole("button", { name: "OK" }).click();
  await expect(list.getByTestId(`plugin-${ECHO_ID}`)).toContainText("decoder");

  // A second install of the same version is refused with the host's reason.
  await page.getByRole("button", { name: "Install from path" }).click();
  const again = page.getByRole("dialog", { name: "Install a plugin" });
  await again.getByLabel("Directory with plugin.toml").fill(ECHO_SRC);
  await again.getByRole("button", { name: "Install", exact: true }).click();
  await expect(again.getByRole("alert")).toBeVisible();
  await again.getByRole("button", { name: "Cancel" }).click();
});

test("an operator starts a decoder on the VFO and sees its state and events", async () => {
  await page.goto("/");
  await expect(page.getByRole("region", { name: "Tuning" })).toBeVisible();
  const panel = await openDecoders();

  await panel.getByRole("button", { name: "Start Example echo" }).click();
  const dialog = page.getByRole("dialog", { name: "Start Example echo" });
  await dialog.getByLabel(/Config \(JSON/).fill('{"mode":"normal"}');
  await dialog.getByRole("button", { name: "Start", exact: true }).click();
  await expect(dialog).toBeHidden();

  // State in the picker and on the VFO tab.
  const running = panel.getByRole("list", { name: "Running on this VFO" });
  await expect(running).toContainText("Example echo");
  await expect(running).toContainText("Running", { timeout: 30_000 });
  await expect(page.getByRole("img", { name: /^Decoders: Example echo/ })).toBeVisible();

  // Its events arrive in the log with the generic view.
  const events = panel.getByRole("list", { name: "Decoder events" });
  await expect(events.getByRole("listitem").first()).toContainText("echo.block", {
    timeout: 45_000,
  });

  // Source filter: the plugin's events, then the built-in detectors (none).
  const source = panel.getByRole("combobox", { name: "Source" });
  await source.selectOption({ label: "Example echo" });
  await expect(events.getByRole("listitem").first()).toContainText("echo.block");
  await source.selectOption({ label: "Built-in detectors" });
  await expect(panel.getByText("No matching events")).toBeVisible();
  await source.selectOption({ label: "All sources" });
  await expect(events.getByRole("listitem").first()).toContainText("echo.block");

  // Details of an event.
  await panel.getByRole("button", { name: "Event details" }).first().click();
  const details = page.getByRole("dialog");
  await expect(details).toContainText("Raw data");
  await expect(details).toContainText(ECHO_ID);
  await details.getByRole("button", { name: "Close", exact: true }).first().click();
  await expect(details).toBeHidden();

  // CSV export carries the source column.
  await panel.getByRole("button", { name: "More log actions" }).click();
  const download = page.waitForEvent("download");
  await page.getByRole("menuitem", { name: "Export CSV" }).click();
  const file = await (await download).path();
  const csv = await readFile(file, "utf8");
  expect(csv.split("\r\n")[0]).toBe("time,ts_ms,device,vfo,freq_hz,mode,kind,value,meta,source");
  expect(csv).toContain(`echo.block`);
  expect(csv).toContain(ECHO_ID);
});

test("the admin console lists the instance and stops it", async () => {
  await page.goto("/admin/plugins");
  const table = page.getByRole("table", { name: "Plugin instances" });
  const row = table.getByRole("row").filter({ hasText: "Example echo" });
  await expect(row).toContainText("Running", { timeout: 30_000 });
  await expect(row).toContainText("VFO");
  await row.getByRole("button", { name: /^Stop / }).click();
  await expect(page.getByText("Nothing is running")).toBeVisible({ timeout: 15_000 });

  // A plugin with no instance can be removed again.
  await page.getByRole("button", { name: "Remove Example echo" }).click();
  await page
    .getByRole("dialog", { name: "Remove Example echo?" })
    .getByRole("button", { name: "Remove", exact: true })
    .click();
  await expect(page.getByTestId(`plugin-${ECHO_ID}`)).toHaveCount(0);
});
