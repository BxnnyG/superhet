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

// What the online services would have said, answered here: the server is never asked, and the
// photo host is answered by the browser context, so nothing leaves the test machine.
const ONLINE_OFF = {
  v: 1,
  enabled: false,
  locked: false,
  sources: { adsbdb: "off", planespotters: "off" },
  aircraft: null,
  route: null,
  photo: null,
};
const ONLINE_ON = {
  v: 1,
  enabled: true,
  locked: false,
  sources: { adsbdb: "ok", planespotters: "ok" },
  aircraft: {
    registration: "D-AIXA",
    type: "A350 941",
    icao_type: "A359",
    manufacturer: "Airbus",
    owner: "Lufthansa",
    owner_country: "Germany",
    owner_country_iso: "DE",
  },
  route: {
    callsign: "DLH400",
    callsign_iata: "LH400",
    airline: { name: "Lufthansa", icao: "DLH", iata: "LH", country: "Germany" },
    origin: {
      name: "Frankfurt am Main Airport",
      icao: "EDDF",
      iata: "FRA",
      city: "Frankfurt am Main",
      country: "Germany",
      country_iso: "DE",
      lat: 50.03333,
      lon: 8.57056,
    },
    destination: {
      name: "John F Kennedy International Airport",
      icao: "KJFK",
      iata: "JFK",
      city: "New York",
      country: "United States",
      country_iso: "US",
      lat: 40.6398,
      lon: -73.7789,
    },
  },
  photo: {
    src: "https://t.plnspttrs.net/05121/e2e_280.jpg",
    link: "https://www.planespotters.net/photo/1/d-aixa-lufthansa-airbus-a350-941",
    photographer: "Example Photographer",
    width: 395,
    height: 280,
  },
};
const HISTORY = {
  v: 1,
  seen: true,
  events: 14,
  days: 3,
  first_ms: Date.now() - 3 * 86_400_000,
  last_ms: Date.now() - 2 * 3_600_000,
  callsigns: [
    { callsign: "DLH400", events: 9, last_ms: Date.now() - 2 * 3_600_000 },
    { callsign: "DLH401", events: 5, last_ms: Date.now() - 86_400_000 },
  ],
};
// A sky with a plane, standing in for the photo.
const PHOTO_SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="395" height="280" viewBox="0 0 395 280">
<defs><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2b6cb0"/><stop offset="1" stop-color="#bee3f8"/></linearGradient></defs>
<rect width="395" height="280" fill="url(#s)"/>
<path d="M60 150 L330 120 L350 128 L330 136 L250 142 L290 200 L270 204 L200 150 L120 158 L90 175 L80 170 L96 156 L60 158 Z" fill="#f7fafc"/></svg>`;

test("the aircraft panel: enable online details, then photo, route, charts and seen-before", async () => {
  let online: unknown = ONLINE_OFF;
  let polls = 0;
  const onlineRequests: string[] = [];
  await context.route("https://t.plnspttrs.net/**", (route) =>
    route.fulfill({ status: 200, contentType: "image/svg+xml", body: PHOTO_SVG }),
  );
  await page.route(/\/api\/v1\/plugin-instances\/[^/]+\/status$/, (route) =>
    route.fulfill({ json: fleetReply(polls++) }),
  );
  await page.route(/\/api\/v1\/adsb\/lookup\?/, (route) => route.fulfill({ json: fleetLookup() }));
  await page.route(/\/api\/v1\/adsb\/online\?/, (route) => {
    onlineRequests.push(route.request().url());
    return route.fulfill({ json: online });
  });
  await page.route(/\/api\/v1\/adsb\/history\?/, (route) => route.fulfill({ json: HISTORY }));

  const view = page.getByRole("dialog", { name: "ADS-B aircraft" });
  await expect(view.getByTestId("adsb-row-3c64a1")).toBeVisible({ timeout: 20_000 });
  // The previous scenario left this aircraft selected; a second click would deselect it.
  if ((await view.getByTestId("adsb-details").count()) === 0) {
    await view.getByTestId("adsb-row-3c64a1").click();
  }
  const details = view.getByTestId("adsb-details");

  // Off by default: the local facts are there, and the admin gets the action with the explanation.
  await expect(details).toContainText("DLH400");
  await expect(details.getByTestId("adsb-online-off")).toContainText("Photo, route and full type");
  expect(onlineRequests.every((u) => !/lat|lon|receiver/i.test(u))).toBe(true);
  await details.getByRole("button", { name: "Enable online details" }).click();
  const dialog = page.getByRole("dialog", { name: "Enable online details?" });
  await expect(dialog).toContainText("Planespotters.net");
  await expect(dialog).toContainText("not the receiver position");
  const saved = page.waitForResponse(
    (r) => r.url().endsWith("/api/v1/admin/adsb/online") && r.request().method() === "PUT",
  );
  await dialog.getByRole("button", { name: "Enable" }).click();
  expect((await saved).status()).toBe(200);

  // The page's CSP only learns the photo host on the next load: reload, reopen the overlay.
  online = ONLINE_ON;
  await page.reload();
  await expect(page.getByRole("region", { name: "Tuning" })).toBeVisible();
  await page.getByRole("tab", { name: "ADS-B", exact: true }).click();
  await page
    .getByTestId("adsb-panel")
    .getByRole("button", { name: "Open the aircraft map and table" })
    .click();
  await expect(view.getByTestId("adsb-row-3c64a1")).toBeVisible({ timeout: 20_000 });
  await view.getByTestId("adsb-row-3c64a1").click();

  // Seen before, from the (here faked) decoder history; answers from before the reload are gone.
  await expect(details.getByTestId("adsb-seen")).toContainText("14 history entries on 3 days");
  const photo = details.getByTestId("adsb-photo");
  await expect(photo).toContainText("Photo: Example Photographer");
  const img = photo.getByRole("img");
  await expect(img).toHaveAttribute("src", "https://t.plnspttrs.net/05121/e2e_280.jpg");
  // The image really loaded (not blocked by the page's policy).
  await expect
    .poll(() => img.evaluate((el: HTMLImageElement) => el.complete && el.naturalWidth > 0))
    .toBe(true);
  await expect(details.getByTestId("adsb-identity")).toContainText("Lufthansa");
  await expect(details.getByTestId("adsb-identity")).toContainText("Airbus A350 941");
  const route = details.getByTestId("adsb-route");
  await expect(route).toContainText("FRA");
  await expect(route).toContainText("JFK");
  await expect(route).toContainText("It can be wrong");
  await expect(details.getByTestId("adsb-route-progress")).toContainText("to go");
  // Charts fill once a few five-second samples are in.
  await expect(details.getByTestId("adsb-chart-altitude").getByRole("img")).toBeVisible({
    timeout: 30_000,
  });
  expect(onlineRequests.length).toBeGreaterThan(0);
  expect(onlineRequests[0]).toMatch(/icao=3c64a1&callsign=DLH400$/);

  // Desktop and phone pictures.
  await page.emulateMedia({ colorScheme: "light" });
  await details.scrollIntoViewIfNeeded();
  await page.screenshot({ path: "e2e/artifacts/adsb-panel-desktop.png" });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(details).toBeVisible();
  await expect(details.getByTestId("adsb-route")).toBeVisible();
  await page.screenshot({ path: "e2e/artifacts/adsb-panel-phone.png" });
  await page.setViewportSize({ width: 1280, height: 720 });

  await page.unroute(/\/api\/v1\/plugin-instances\/[^/]+\/status$/);
  await page.unroute(/\/api\/v1\/adsb\/lookup\?/);
  await page.unroute(/\/api\/v1\/adsb\/online\?/);
  await page.unroute(/\/api\/v1\/adsb\/history\?/);
  await context.unroute("https://t.plnspttrs.net/**");

  // Back to the default: nothing may ask a real service from this stack.
  const off = await page.evaluate(async () => {
    const res = await fetch("/api/v1/admin/adsb/online", {
      method: "PUT",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ enabled: false, adsbdb: false, planespotters: false }),
    });
    return res.status;
  });
  expect(off).toBe(200);
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
