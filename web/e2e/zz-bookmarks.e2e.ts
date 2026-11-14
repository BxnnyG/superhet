import { expect, test, type BrowserContext, type Page } from "@playwright/test";

// Browser smoke test of bookmark saving and of moving the centre. The operator starts POCSAG in
// the Digital picker (the stack has a stand-in multimon-ng), bookmarks the frequency (the dialog
// carries the running mode, a double click on Save saves once, saving it again offers an update
// instead of a duplicate), goes back to analogue and loads the bookmark: POCSAG starts again.
// Then the centre is moved with the drag bar above the waterfall and the wheel over the centre
// pill, and a locked tuning says why it does not move. The login of the earlier scenarios is reused.

const SETUP_TOKEN = process.env.E2E_SETUP_TOKEN ?? "e2e-setup-token-0123456789";
const ADMIN = { username: "root", password: "admin passphrase for the smoke test" };
const STATE = "/api/v1/devices/dev0/state";
const POCSAG = "org.superhet.decoder.pocsag";

test.describe.configure({ mode: "serial" });

let context: BrowserContext;
let page: Page;

test.beforeAll(async ({ browser }) => {
  context = await browser.newContext();
  page = await context.newPage();
});

test.afterAll(async () => {
  await api("DELETE", "/api/v1/devices/dev0/lock").catch(() => undefined);
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

async function api(method: string, url: string, body?: unknown) {
  return page.evaluate(
    async ([m, u, b]) => {
      const res = await fetch(u as string, {
        method: m as string,
        headers: b === undefined ? {} : { "Content-Type": "application/json" },
        body: b === undefined ? undefined : JSON.stringify(b),
      });
      const text = await res.text();
      return { status: res.status, body: text === "" ? null : JSON.parse(text) };
    },
    [method, url, body] as const,
  );
}

const centre = async (): Promise<number> => (await api("GET", STATE)).body.center_hz as number;
const bookmarks = async () =>
  (await api("GET", "/api/v1/bookmarks")).body.bookmarks as {
    id: string;
    name: string;
    freq_hz: number;
    digital: { protocol: string } | null;
    squelch_db: number | null;
  }[];
const pocsagInstances = async () =>
  (
    (await api("GET", "/api/v1/plugin-instances")).body.instances as {
      plugin_id: string;
    }[]
  ).filter((i) => i.plugin_id === POCSAG);

test("a bookmark saves the running digital mode once and offers an update instead of a duplicate", async () => {
  await signInAsAdmin();
  const state = await api("GET", STATE);
  if ((state.body.vfos as unknown[]).length === 0) {
    const made = await api("POST", "/api/v1/devices/dev0/vfos", {
      freq_hz: state.body.center_hz,
      mode: "nfm",
    });
    expect(made.status).toBe(201);
    await page.reload();
    await expect(page.getByRole("region", { name: "Tuning" })).toBeVisible();
  }
  for (const b of await bookmarks()) await api("DELETE", `/api/v1/bookmarks/${b.id}`);

  // POCSAG from the Digital picker, as a user would start it.
  await page.getByRole("combobox", { name: "Digital mode" }).selectOption({ label: "POCSAG" });
  const notice = page.getByRole("dialog", { name: "Start POCSAG" });
  await notice.getByRole("switch").click();
  await notice.getByRole("button", { name: "Start POCSAG" }).click();
  await expect(notice).toBeHidden();
  await expect.poll(async () => (await pocsagInstances()).length).toBe(1);

  // The dialog takes the state of the VFO, digital mode included; a double click saves once.
  await page.getByRole("button", { name: "Bookmark this frequency" }).click();
  const dialog = page.getByRole("dialog", { name: "Add bookmark" });
  await expect(dialog.getByLabel("Protocol")).toHaveValue("pocsag");
  await expect(dialog.getByLabel("Name")).toHaveValue(/POCSAG/);
  await expect(dialog.getByLabel("Mode")).toHaveValue("nfm");
  await expect(dialog.getByLabel("Bandwidth (kHz)")).toHaveValue("12.5");
  await dialog.getByRole("button", { name: "Save" }).dblclick();
  await expect(dialog).toBeHidden();
  await expect.poll(async () => (await bookmarks()).length).toBe(1);
  await page.waitForTimeout(500);
  expect(await bookmarks()).toHaveLength(1);
  const saved = (await bookmarks())[0];
  expect(saved?.digital?.protocol).toBe("pocsag");

  // The same frequency and modes again: ask, then update the existing one.
  await page.getByRole("button", { name: "Bookmark this frequency" }).click();
  const again = page.getByRole("dialog", { name: "Add bookmark" });
  await again.getByLabel("Squelch").selectOption("fixed");
  await again.getByLabel("Squelch level (dBFS)").fill("-70");
  await again.getByRole("button", { name: "Save" }).click();
  await expect(again).toContainText("already stores this channel");
  expect(await bookmarks()).toHaveLength(1);
  await again.getByRole("button", { name: /^Update / }).click();
  await expect(again).toBeHidden();
  await expect.poll(async () => (await bookmarks())[0]?.squelch_db).toBe(-70);
  expect(await bookmarks()).toHaveLength(1);
});

test("loading a digital bookmark starts that mode again", async () => {
  // Back to an analogue mode: the decoder stops.
  const mode = page.getByRole("radiogroup", { name: "Mode" });
  await mode.getByRole("radio", { name: "WFM" }).click();
  await expect.poll(async () => (await pocsagInstances()).length).toBe(0);

  await page.getByRole("tab", { name: "Bookmarks" }).click();
  const name = (await bookmarks())[0]?.name ?? "";
  await page
    .getByRole("button", { name: `Tune to ${name}` })
    .first()
    .click();
  await expect.poll(async () => (await pocsagInstances()).length, { timeout: 20_000 }).toBe(1);
  await expect(mode.getByRole("radio", { name: "NFM" })).toBeChecked();
  await expect(page.getByTestId("digital-dock")).toContainText("Running", { timeout: 30_000 });

  // Leave nothing running for the scenarios that follow.
  await mode.getByRole("radio", { name: "WFM" }).click();
  await expect.poll(async () => (await pocsagInstances()).length).toBe(0);
  for (const b of await bookmarks()) await api("DELETE", `/api/v1/bookmarks/${b.id}`);
});

test("the drag bar and the wheel over the centre pill move the centre", async () => {
  const before = await centre();
  const bar = page.getByRole("button", { name: /Move the device centre/ });
  const box = await bar.boundingBox();
  expect(box).not.toBeNull();
  const x = (box?.x ?? 0) + (box?.width ?? 0) / 2;
  const y = (box?.y ?? 0) + (box?.height ?? 0) / 2;

  // Dragging the picture to the right moves the centre down; nothing is sent before the release.
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 60, y, { steps: 6 });
  await page.mouse.move(x + 150, y, { steps: 6 });
  await expect(page.getByTestId("centre-bar-chip")).toBeVisible();
  expect(await centre()).toBe(before);
  await page.mouse.up();
  await expect.poll(centre).toBeLessThan(before);
  const moved = await centre();

  // The wheel over the pill steps the centre: up goes up.
  const pill = page.getByRole("button", { name: /^Centre / });
  await pill.hover();
  await page.mouse.wheel(0, -120);
  await expect.poll(centre).toBeGreaterThan(moved);

  // Arrow keys on the focused pill.
  const wheeled = await centre();
  await pill.focus();
  await page.keyboard.press("ArrowLeft");
  await expect.poll(centre).toBeLessThan(wheeled);
});

test("a locked tuning says why the centre does not move", async () => {
  const locked = await api("POST", "/api/v1/devices/dev0/lock");
  expect(locked.status).toBe(200);
  await expect(page.getByRole("button", { name: /^Centre .*locked/ })).toBeVisible();
  const before = await centre();

  const bar = page.getByRole("button", { name: /Move the device centre/ });
  const box = await bar.boundingBox();
  const x = (box?.x ?? 0) + (box?.width ?? 0) / 2;
  const y = (box?.y ?? 0) + (box?.height ?? 0) / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 120, y, { steps: 8 });
  await page.mouse.up();
  await expect(page.getByText("Tuning is locked")).toBeVisible();
  await expect(page.getByTestId("centre-bar-chip")).toBeHidden();
  expect(await centre()).toBe(before);

  await api("DELETE", "/api/v1/devices/dev0/lock");
});
