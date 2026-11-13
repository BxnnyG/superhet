import { expect, test, type BrowserContext, type Page } from "@playwright/test";
import { FLEET, fleetLookup, fleetReply } from "./adsb-fleet";

// Browser smoke test of the ADS-B plugin (docs/plugins-adsb.md): the operator starts the bundled
// plugin through the ADS-B tab (takeover confirmation), the map and table overlay opens with the
// aircraft, a row can be selected, and everything stops again. The plugin is the real adapter
// around a stand-in readsb (crates/superhet-plugin-adapters/tests/adsb/fake-readsb, python3) that
// serves two recorded aircraft.json files; the device is the file backend of
// scripts/e2e-stack.sh. OpenStreetMap is never contacted: the tile requests are answered here.
// File name sorts before zz-dab so the two exclusive plugins take turns on the device.

const SETUP_TOKEN = process.env.E2E_SETUP_TOKEN ?? "e2e-setup-token-0123456789";
const ADMIN = { username: "root", password: "admin passphrase for the smoke test" };

// A 1x1 transparent PNG.
const PIXEL = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
  "base64",
);

test.describe.configure({ mode: "serial" });

let context: BrowserContext;
let page: Page;
const tiles: string[] = [];

test.beforeAll(async ({ browser }) => {
  context = await browser.newContext();
  await context.route("https://tile.openstreetmap.org/**", (route) => {
    tiles.push(route.request().url());
    return route.fulfill({ status: 200, contentType: "image/png", body: PIXEL });
  });
  page = await context.newPage();
});

test.afterAll(async () => {
  await context.close();
});

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

test("the operator starts ADS-B and sees the aircraft on the map and in the table", async () => {
  await signInAsAdmin();
  await page.getByRole("tab", { name: "ADS-B", exact: true }).click();
  const panel = page.getByTestId("adsb-panel");
  await expect(panel.getByText("Track aircraft on 1090 MHz")).toBeVisible();

  await panel.getByRole("button", { name: "Start ADS-B…" }).click();
  const confirm = page.getByRole("dialog", { name: "Start ADS-B on 1090 MHz?" });
  await expect(confirm).toContainText("takes the whole device");
  await confirm.getByLabel("Latitude").fill("51.0");
  await confirm.getByLabel("Longitude").fill("7.0");
  await confirm.getByRole("button", { name: "Start ADS-B" }).click();

  // The plugin claims the device, the stand-in readsb "sees" the recorded aircraft and the
  // overlay opens by itself. Once IQ arrives it switches to the second recording.
  const view = page.getByRole("dialog", { name: "ADS-B aircraft" });
  await expect(view).toBeVisible({ timeout: 60_000 });
  const table = view.getByRole("region", { name: "Aircraft table" });
  await expect(table.getByTestId("adsb-row-3c6444")).toContainText("DLH123", { timeout: 60_000 });
  await expect(table.getByTestId("adsb-row-4ca7b2")).toContainText("RYR8QK", { timeout: 60_000 });
  await expect(view.getByText("2 on map")).toBeVisible();

  // The map: Leaflet with a marker per aircraft, the station, and OSM's attribution.
  const map = view.getByRole("application", { name: "Aircraft map" });
  await expect(map.locator(".leaflet-marker-icon")).toHaveCount(2);
  await expect(map.locator(".leaflet-control-attribution")).toContainText("OpenStreetMap");
  await expect.poll(() => tiles.length, { timeout: 15_000 }).toBeGreaterThan(0);
  expect(tiles[0]).toMatch(/^https:\/\/tile\.openstreetmap\.org\/\d+\/\d+\/\d+\.png$/);

  // Selecting a row shows the details of the aircraft and marks it on the map.
  await table.getByTestId("adsb-row-3c6444").click();
  await expect(view.getByTestId("adsb-details")).toContainText("DLH123");
  await expect(map.locator(".adsb-ring:not([hidden])")).toHaveCount(1);
});

test("a mixed fleet gets the right symbols, units, trails, filters and emergency marks", async () => {
  // The same overlay, fed with one aircraft of every kind (status and lookup are answered here).
  let polls = 0;
  await page.route(/\/api\/v1\/plugin-instances\/[^/]+\/status$/, (route) =>
    route.fulfill({ json: fleetReply(polls++) }),
  );
  await page.route(/\/api\/v1\/adsb\/lookup\?/, (route) => route.fulfill({ json: fleetLookup() }));
  await page.evaluate(() => window.localStorage.removeItem("superhet.adsb.units.v1"));

  const view = page.getByRole("dialog", { name: "ADS-B aircraft" });
  const map = view.getByRole("application", { name: "Aircraft map" });
  await expect(map.locator(".leaflet-marker-icon")).toHaveCount(FLEET.length, { timeout: 20_000 });

  // Every aircraft has the symbol of its type; none is drawn as a jet by default.
  for (const f of FLEET) {
    const title = f.callsign ?? f.icao;
    await expect(map.locator(`.adsb-plane[title^="${title}"]`)).toHaveAttribute(
      "data-kind",
      f.kind,
    );
  }

  // Metric by default, the other unit in the tooltip.
  const dlh = view.getByTestId("adsb-row-3c64a1");
  await expect(dlh).toContainText("11,582 m");
  await expect(dlh).toContainText("907 km/h");
  await expect(dlh.getByTitle("490 kt")).toBeVisible();

  // The emergency squawk is marked in the header, the table and on the map.
  await expect(view.getByTestId("adsb-emergency-3e1a0a")).toContainText("7700 EMG777");
  await expect(map.locator(".adsb-ring[data-emergency]")).toHaveCount(1);

  // Details: identity, GNSS altitude, speed, distance and bearing from the receiver.
  await view.getByTestId("adsb-row-3e1a03").click();
  const details = view.getByTestId("adsb-details");
  await expect(details).toContainText("CHX33");
  await expect(details).toContainText("Helicopter");
  await expect(details).toContainText("EUROCOPTER EC-135");
  await expect(details).toContainText("Altitude (GNSS)");
  await expect(details).toContainText("Distance, bearing");
  await expect(details.getByRole("link", { name: "ADS-B Exchange" })).toBeVisible();

  // Aviation units on request; the choice survives a reload of the overlay.
  await view.getByRole("button", { name: "Map", exact: true }).click();
  await view.getByLabel("Units").selectOption("aviation");
  await expect(dlh).toContainText("38,000 ft");
  await expect(dlh).toContainText("490 kt");
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem("superhet.adsb.units.v1")))
    .toBe("aviation");
  await view.getByLabel("Units").selectOption("metric");
  await view.getByLabel("Trails").selectOption("5");
  await view.getByRole("switch", { name: /Range rings/ }).click();
  await expect(map.locator("path.adsb-range-ring").first()).toBeVisible();
  await view.getByRole("button", { name: "Map", exact: true }).click();

  // The table sorts by distance from the receiver (nearest first).
  await view.getByRole("button", { name: "Sort by Distance" }).click();
  await expect(view.getByTestId(/^adsb-row-/).first()).toHaveAttribute(
    "data-testid",
    "adsb-row-3e1a03",
  );

  // Filters: hide helicopters; only military.
  await view.getByRole("button", { name: /^Filter/ }).click();
  await view.getByLabel("Helicopters").uncheck();
  await expect(view.getByTestId("adsb-row-3e1a03")).toHaveCount(0);
  await expect(map.locator(".adsb-plane")).toHaveCount(FLEET.length - 1);
  await view.getByRole("button", { name: "Clear filters" }).click();
  await view.getByRole("switch", { name: "Military only" }).click();
  await expect(map.locator(".adsb-plane")).toHaveCount(1);
  await view.getByRole("button", { name: "Clear filters" }).click();
  await expect(map.locator(".adsb-plane")).toHaveCount(FLEET.length);
  await view.getByRole("button", { name: /^Filter/ }).click();

  // Let the trails grow, then take the pictures (light and dark theme).
  await view.getByTestId("adsb-row-3c64a1").click();
  await page.waitForTimeout(9_000);
  await expect(map.locator("canvas.adsb-trail-canvas")).toHaveCount(1);
  await view.getByRole("button", { name: "Fit map" }).click();
  await page.emulateMedia({ colorScheme: "light" });
  await page.screenshot({ path: "e2e/artifacts/adsb-map-light.png" });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.screenshot({ path: "e2e/artifacts/adsb-map-dark.png" });
  await page.emulateMedia({ colorScheme: "light" });

  await page.unroute(/\/api\/v1\/plugin-instances\/[^/]+\/status$/);
  await page.unroute(/\/api\/v1\/adsb\/lookup\?/);
});

test("the view closes, the tab keeps the numbers, and ADS-B stops cleanly", async () => {
  await page.getByRole("button", { name: "Close the ADS-B view" }).click();
  await expect(page.getByRole("dialog", { name: "ADS-B aircraft" })).toBeHidden();

  const panel = page.getByTestId("adsb-panel");
  await expect(panel.getByTestId("adsb-summary")).toContainText("Aircraft");
  await panel.getByRole("button", { name: "Open the aircraft map and table" }).click();
  await expect(page.getByRole("dialog", { name: "ADS-B aircraft" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "ADS-B aircraft" })).toBeHidden();

  await panel.getByRole("button", { name: /Stop ADS-B/ }).click();
  await expect(panel.getByRole("button", { name: "Start ADS-B…" })).toBeVisible({
    timeout: 30_000,
  });
  await page.goto("/admin/plugins");
  await expect(page.getByText("Nothing is running")).toBeVisible({ timeout: 15_000 });
});
