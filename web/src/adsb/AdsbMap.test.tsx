import { act, fireEvent, render, screen } from "@testing-library/react";
import L from "leaflet";
import { describe, expect, it, vi } from "vitest";
import { TILE_REFERRER_POLICY } from "../map/BaseMap";
import { AdsbMap } from "./AdsbMap";
import { DEFAULT_MAP } from "../map/mapConfig";
import { parseStatus, tracked, type Tracked } from "./model";
import { adsbStatusRaw } from "./testing";
import { SYMBOL_KINDS, SYMBOL_SIZE, type SymbolKind } from "./symbols";

// Leaflet runs in jsdom without layout (the map is 0 x 0 pixels), which is enough to check that
// the right markers, labels and tiles exist.
function picture(ageS = 0): Tracked[] {
  const status = parseStatus(adsbStatusRaw());
  if (!status) throw new Error("fixture");
  return tracked(status, 0, ageS * 1000).filter((a) => a.lat !== null);
}

const props = {
  receiver: { lat: 51, lon: 7 },
  selected: null,
  onSelect: () => {},
  fitSignal: 0,
  tiles: DEFAULT_MAP,
};

describe("AdsbMap", () => {
  it("uses OpenStreetMap tiles with the attribution the tile policy asks for", () => {
    const layer = vi.spyOn(L, "tileLayer");
    const { container } = render(<AdsbMap aircraft={picture()} {...props} />);
    expect(layer.mock.calls[0]?.[0]).toBe("https://tile.openstreetmap.org/{z}/{x}/{y}.png");
    const attribution = container.querySelector(".leaflet-control-attribution");
    expect(attribution).toHaveTextContent("OpenStreetMap contributors");
    expect(attribution?.querySelector('a[href*="openstreetmap"]')).toHaveAttribute(
      "href",
      "https://www.openstreetmap.org/copyright",
    );
    layer.mockRestore();
  });

  it("sends the page origin as Referer with tile requests (OSM blocks clients without one)", () => {
    const layer = vi.spyOn(L, "tileLayer");
    render(<AdsbMap aircraft={picture()} {...props} />);
    const options = layer.mock.calls[0]?.[1];
    expect(TILE_REFERRER_POLICY).toBe("strict-origin-when-cross-origin");
    expect(options?.referrerPolicy).toBe(TILE_REFERRER_POLICY);
    // A failed tile must not leave a broken-image grid behind.
    expect(options?.errorTileUrl).toMatch(/^data:image\//);
    layer.mockRestore();
  });

  it("loads tiles from the configured server and shows its attribution as text", () => {
    const layer = vi.spyOn(L, "tileLayer");
    const tiles = {
      tileUrl: "http://tiles.lan:8080/{z}/{x}/{y}.png",
      proxied: false,
      attribution: "Own <b>tiles</b>",
      attributionUrl: null,
    };
    const { container } = render(<AdsbMap aircraft={picture()} {...props} tiles={tiles} />);
    expect(layer.mock.calls[0]?.[0]).toBe(tiles.tileUrl);
    const attribution = container.querySelector(".leaflet-control-attribution");
    expect(attribution?.querySelector("b")).toBeNull();
    expect(attribution).toHaveTextContent("Own <b>tiles</b>");
    layer.mockRestore();
  });

  it("explains a refusing tile server instead of leaving a grid of broken tiles", () => {
    const layer = vi.spyOn(L, "tileLayer");
    render(<AdsbMap aircraft={picture()} {...props} />);
    const tileLayer = layer.mock.results[0]?.value as L.TileLayer;
    const problem = /map background could not be loaded from tile\.openstreetmap\.org/;
    expect(screen.queryByText(problem)).toBeNull();
    act(() => {
      for (let i = 0; i < 3; i++) tileLayer.fire("tileerror");
    });
    expect(screen.getByText(problem)).toBeInTheDocument();
    // The first tile that does load clears it.
    act(() => {
      tileLayer.fire("tileload");
    });
    expect(screen.queryByText(problem)).toBeNull();
    layer.mockRestore();
  });

  it("draws a marker per aircraft with a position and one for the receiver", () => {
    const { container } = render(<AdsbMap aircraft={picture()} {...props} />);
    expect(container.querySelectorAll(".leaflet-marker-icon")).toHaveLength(2);
    expect(container.querySelectorAll("path.leaflet-interactive")).toHaveLength(1);
  });

  it("labels aircraft with the callsign, and with altitude and speed when asked", () => {
    const { container, rerender } = render(<AdsbMap aircraft={picture()} {...props} />);
    const l1 = () => [...container.querySelectorAll(".adsb-l1")].map((e) => e.textContent).sort();
    expect(l1()).toEqual(["DLH123", "EZY45K"]);
    expect(container.querySelectorAll(".adsb-l2:not(:empty)")).toHaveLength(0);
    const fields = { callsign: true, altitude: true, speed: true };
    rerender(<AdsbMap aircraft={picture()} {...props} labels={fields} units="metric" />);
    expect([...container.querySelectorAll(".adsb-l2")].map((e) => e.textContent).sort()).toEqual([
      "10,668 m 836 km/h",
      "ground 22 km/h",
    ]);
    rerender(<AdsbMap aircraft={picture()} {...props} labels={fields} units="aviation" />);
    expect([...container.querySelectorAll(".adsb-l2")].map((e) => e.textContent).sort()).toEqual([
      "35,000 ft 451 kt",
      "ground 12 kt",
    ]);
    rerender(
      <AdsbMap
        aircraft={picture()}
        {...props}
        labels={{ callsign: false, altitude: false, speed: false }}
      />,
    );
    expect(l1()).toEqual(["", ""]);
  });

  it("shows the value in the other unit as the tooltip of the symbol", () => {
    const { container } = render(<AdsbMap aircraft={picture()} {...props} units="metric" />);
    const titles = [...container.querySelectorAll<HTMLElement>(".adsb-plane")].map((e) => e.title);
    expect(titles.find((t) => t.startsWith("DLH123"))).toBe(
      "DLH123 \u00b7 10,668 m (35,000 ft) \u00b7 836 km/h (451 kt)",
    );
  });

  it("fades markers with the aircraft and removes the ones that left", () => {
    const { container, rerender } = render(<AdsbMap aircraft={picture()} {...props} />);
    const first = container.querySelector<HTMLElement>(".leaflet-marker-icon");
    expect(first?.style.opacity).toBe("1");
    rerender(<AdsbMap aircraft={picture(45).slice(0, 1)} {...props} />);
    const icons = container.querySelectorAll<HTMLElement>(".leaflet-marker-icon");
    expect(icons).toHaveLength(1);
    expect(Number(icons[0]?.style.opacity)).toBeLessThan(0.6);
    rerender(<AdsbMap aircraft={[]} {...props} />);
    expect(container.querySelectorAll(".leaflet-marker-icon")).toHaveLength(0);
    expect(container.querySelectorAll(".adsb-tag")).toHaveLength(0);
  });

  it("marks the selected aircraft and reports clicks", () => {
    const onSelect = vi.fn();
    const { container, rerender } = render(
      <AdsbMap aircraft={picture()} {...props} onSelect={onSelect} />,
    );
    expect([...container.querySelectorAll<HTMLElement>(".adsb-ring")].every((e) => e.hidden)).toBe(
      true,
    );
    const icon = container.querySelector(".leaflet-marker-icon");
    if (!icon) throw new Error("no marker");
    fireEvent.click(icon);
    expect(onSelect).toHaveBeenCalledWith("3c6444");
    rerender(<AdsbMap aircraft={picture()} {...props} selected="3c6444" onSelect={onSelect} />);
    expect(container.querySelectorAll(".adsb-ring:not([hidden])")).toHaveLength(1);
  });

  it("keeps the marker element when the aircraft moves and turns the short way", () => {
    const { container, rerender } = render(<AdsbMap aircraft={picture()} {...props} />);
    const icon = container.querySelector(".leaflet-marker-icon");
    const moved = picture().map((a) =>
      a.icao === "3c6444" ? { ...a, lat: 51.2, lon: 7.1, trackDeg: 355 } : a,
    );
    rerender(<AdsbMap aircraft={moved} {...props} />);
    expect(container.querySelectorAll(".leaflet-marker-icon")[0]).toBe(icon);
    expect(container.querySelector<HTMLElement>(".adsb-plane")?.style.transform).toBe(
      "rotate(-5deg)",
    );
    const north = moved.map((a) => (a.icao === "3c6444" ? { ...a, trackDeg: 5 } : a));
    rerender(<AdsbMap aircraft={north} {...props} />);
    // 355 -> 5 degrees is a turn of 10 degrees (-5 -> 5), not 350 back.
    expect(container.querySelector<HTMLElement>(".adsb-plane")?.style.transform).toBe(
      "rotate(5deg)",
    );
  });

  it("draws trails on a canvas under the symbols, not as thousands of paths", () => {
    const { container } = render(<AdsbMap aircraft={picture()} {...props} trailMinutes={5} />);
    expect(container.querySelectorAll(".leaflet-adsb-trails-pane canvas")).toHaveLength(1);
    // Only the receiver is an SVG path.
    expect(container.querySelectorAll(".leaflet-overlay-pane path")).toHaveLength(1);
  });

  it("draws range rings in the chosen unit and removes them", () => {
    const { container, rerender } = render(<AdsbMap aircraft={picture()} {...props} rings />);
    expect(container.querySelectorAll("path.adsb-range-ring").length).toBeGreaterThan(0);
    const text = container.querySelector(".adsb-ring-label")?.textContent ?? "";
    expect(text).toMatch(/ km$/);
    rerender(<AdsbMap aircraft={picture()} {...props} rings units="aviation" />);
    expect(container.querySelector(".adsb-ring-label")?.textContent).toMatch(/ NM$/);
    rerender(<AdsbMap aircraft={picture()} {...props} rings={false} />);
    expect(container.querySelectorAll("path.adsb-range-ring")).toHaveLength(0);
  });

  it("draws the symbol of the aircraft type, sized by class", () => {
    const symbols = new Map<string, SymbolKind>([
      ["3c6444", "wide"],
      ["406a3d", "heli"],
    ]);
    const { container } = render(<AdsbMap aircraft={picture()} {...props} symbols={symbols} />);
    const kinds = [...container.querySelectorAll(".adsb-plane")].map((e) =>
      e.getAttribute("data-kind"),
    );
    expect(kinds.sort()).toEqual(["heli", "wide"]);
    const sizes = new Map(
      [...container.querySelectorAll<HTMLElement>(".leaflet-marker-icon")].map((e) => [
        e.querySelector(".adsb-plane")?.getAttribute("data-kind"),
        e.style.width,
      ]),
    );
    expect(sizes.get("wide")).toBe(`${SYMBOL_SIZE.wide}px`);
    expect(sizes.get("heli")).toBe(`${SYMBOL_SIZE.heli}px`);
    // A changed type swaps the symbol.
    symbols.set("3c6444", "light");
    const { container: c2 } = render(
      <AdsbMap aircraft={picture()} {...props} symbols={new Map([["3c6444", "light"]])} />,
    );
    expect(c2.querySelector('.adsb-plane[data-kind="light"]')).not.toBeNull();
    // Every kind has a shape, so no aircraft is ever drawn as nothing.
    for (const kind of SYMBOL_KINDS) {
      const { container: c } = render(
        <AdsbMap
          aircraft={picture().slice(0, 1)}
          {...props}
          symbols={new Map([["3c6444", kind]])}
        />,
      );
      expect(c.querySelector(".adsb-plane svg")?.innerHTML.length).toBeGreaterThan(20);
    }
  });

  it("marks an emergency squawk on the map", () => {
    const sos = picture().map((a) => (a.icao === "3c6444" ? { ...a, squawk: "7700" } : a));
    const { container } = render(<AdsbMap aircraft={sos} {...props} />);
    const rings = [...container.querySelectorAll<HTMLElement>(".adsb-ring")].filter(
      (e) => !e.hidden,
    );
    expect(rings).toHaveLength(1);
    expect(rings[0]?.dataset.emergency).toBe("true");
  });

  it("draws and removes the coverage outline", () => {
    const ring: [number, number][] = [
      [52, 7],
      [51, 8],
      [50, 7],
      [51, 6],
    ];
    const { container, rerender } = render(
      <AdsbMap aircraft={picture()} {...props} coverage={ring} />,
    );
    expect(container.querySelectorAll("path.adsb-coverage")).toHaveLength(1);
    rerender(<AdsbMap aircraft={picture()} {...props} coverage={null} />);
    expect(container.querySelectorAll("path.adsb-coverage")).toHaveLength(0);
  });
});
