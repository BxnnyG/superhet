import { expect, test, type BrowserContext, type Page } from "@playwright/test";

// Browser smoke test of the DAB plugin (docs/plugins-dab.md): the operator starts the bundled DAB
// plugin on a block through the DAB tab (takeover confirmation), sees the ensemble, the services,
// plays one (audio frames reach the audio WebSocket, station text and slide arrive), runs into a
// scan and stops everything again. The plugin is the real adapter around a stand-in welle-cli
// (crates/superhet-plugin-adapters/tests/dab/fake-welle-cli, python3), the device is the file
// backend of scripts/e2e-stack.sh. Runs last, after the plugin scenario (file name).

const SETUP_TOKEN = process.env.E2E_SETUP_TOKEN ?? "e2e-setup-token-0123456789";
const ADMIN = { username: "root", password: "admin passphrase for the smoke test" };

test.describe.configure({ mode: "serial" });

let context: BrowserContext;
let page: Page;

test.beforeAll(async ({ browser }) => {
  context = await browser.newContext();
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

test("the operator starts DAB on a block and sees the ensemble and its services", async () => {
  await signInAsAdmin();
  await page.getByRole("tab", { name: "DAB", exact: true }).click();
  const panel = page.getByTestId("dab-panel");
  await expect(panel.getByText("Receive DAB / DAB+")).toBeVisible();

  await panel.getByLabel("Block").selectOption("5C");
  await panel.getByRole("button", { name: "Start DAB…" }).click();
  const confirm = page.getByRole("dialog", { name: "Start DAB on 5C?" });
  await expect(confirm).toContainText("takes the whole device");
  await confirm.getByRole("button", { name: "Start DAB" }).click();

  // The plugin starts, claims the device, the stand-in welle-cli "locks" once IQ arrives.
  await expect(panel.getByTestId("dab-ensemble")).toHaveText("DR Deutschland", {
    timeout: 60_000,
  });
  const services = panel.getByRole("list", { name: "Services" });
  await expect(services).toContainText("Deutschlandfunk");
  await expect(services).toContainText("DKultur");
  await expect(panel.getByText("Good signal")).toBeVisible();
});

test("playing a service delivers audio, station text and the slide", async () => {
  const panel = page.getByTestId("dab-panel");
  await panel.getByRole("button", { name: "Play Deutschlandfunk" }).click();
  await expect(panel.getByTestId("dab-dls")).toHaveText("Nachrichten: Bundestag debattiert", {
    timeout: 45_000,
  });
  const slide = panel.getByRole("img", { name: /Slideshow image of Deutschlandfunk/ });
  await expect(slide).toBeVisible();
  const src = await slide.getAttribute("src");
  const media = await page.request.get(src ?? "");
  expect(media.status()).toBe(200);
  expect(media.headers()["content-type"]).toBe("image/png");
  expect((await media.body()).subarray(1, 4).toString()).toBe("PNG");

  // The audio of the instance is a stream on the normal audio socket: Opus frames ("SHAU").
  const frame = await page.evaluate(async () => {
    const ids = (await (await fetch("/api/v1/plugin-instances")).json()).instances as {
      id: string;
      audio_vfo: number | null;
    }[];
    const vfo = ids.find((i) => i.audio_vfo !== null)?.audio_vfo;
    if (vfo == null) return "no audio stream id";
    return await new Promise<string>((resolve) => {
      const ws = new WebSocket(
        `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/api/v1/ws/audio?dev=dev0`,
      );
      ws.binaryType = "arraybuffer";
      const timer = setTimeout(() => resolve("timeout"), 20_000);
      ws.onopen = () => ws.send(JSON.stringify({ type: "subscribe", v: 1, vfos: [vfo] }));
      ws.onmessage = (e) => {
        if (typeof e.data === "string") return;
        const bytes = new Uint8Array(e.data as ArrayBuffer);
        const magic = String.fromCharCode(...bytes.slice(0, 4));
        const id = new DataView(bytes.buffer).getUint32(12, true);
        clearTimeout(timer);
        ws.close();
        resolve(
          `${magic} ${id === vfo ? "ok" : "wrong id"} ${bytes.length > 42 ? "payload" : "empty"}`,
        );
      };
    });
  });
  expect(frame).toBe("SHAU ok payload");
});

test("a scan shows progress, can be stopped, and DAB stops cleanly", async () => {
  const panel = page.getByTestId("dab-panel");
  await panel.getByRole("button", { name: "Scan all blocks" }).click();
  await expect(panel.getByRole("progressbar", { name: "Scan progress" })).toBeVisible({
    timeout: 30_000,
  });
  await expect(panel.getByText(/^Scanning /)).toBeVisible();
  await panel.getByRole("button", { name: "Stop scan" }).click();
  await expect(panel.getByRole("progressbar")).toBeHidden({ timeout: 30_000 });

  await panel.getByRole("button", { name: /Stop DAB/ }).click();
  await expect(panel.getByRole("button", { name: "Start DAB…" })).toBeVisible({ timeout: 30_000 });
  // The instance is gone from the admin console as well.
  await page.goto("/admin/plugins");
  await expect(page.getByText("Nothing is running")).toBeVisible({ timeout: 15_000 });
});
