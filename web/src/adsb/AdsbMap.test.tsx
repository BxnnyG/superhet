import { act, fireEvent, render, screen } from "@testing-library/react";
import L from "leaflet";
import { describe, expect, it, vi } from "vitest";
import { AdsbMap, TILE_REFERRER_POLICY } from "./AdsbMap";
import { DEFAULT_MAP } from "./mapConfig";
import { parseStatus, tracked, type Tracked } from "./model";
import { adsbStatusRaw } from "./testing";

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

  it("draws a labelled marker per aircraft with a position and one for the receiver", () => {
    const { container } = render(<AdsbMap aircraft={picture()} {...props} />);
    expect(container.querySelectorAll(".leaflet-marker-icon")).toHaveLength(2);
    const labels = [...container.querySelectorAll(".adsb-label")].map((e) => e.textContent);
    expect(labels.sort()).toEqual(["DLH123", "EZY45K"]);
    expect(container.querySelectorAll("path.leaflet-interactive")).toHaveLength(1);
  });

  it("rotates the symbol to the track and colours it by altitude", () => {
    const { container } = render(<AdsbMap aircraft={picture()} {...props} />);
    const plane = container.querySelector<HTMLElement>(".adsb-plane");
    expect(plane?.style.transform).toBe("rotate(125deg)");
    expect(plane?.style.color).not.toBe("");
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
    expect(container.querySelectorAll(".adsb-label")).toHaveLength(0);
  });

  it("marks the selected aircraft and reports clicks", () => {
    const onSelect = vi.fn();
    const { container, rerender } = render(
      <AdsbMap aircraft={picture()} {...props} onSelect={onSelect} />,
    );
    expect(container.querySelector(".adsb-ring")).toBeNull();
    const icon = container.querySelector(".leaflet-marker-icon");
    if (!icon) throw new Error("no marker");
    fireEvent.click(icon);
    expect(onSelect).toHaveBeenCalledWith("3c6444");
    rerender(<AdsbMap aircraft={picture()} {...props} selected="3c6444" onSelect={onSelect} />);
    expect(container.querySelectorAll(".adsb-ring")).toHaveLength(1);
  });

  it("draws a trail that grows when the aircraft moves", () => {
    const { container, rerender } = render(<AdsbMap aircraft={picture()} {...props} />);
    const moved = picture().map((a) => (a.icao === "3c6444" ? { ...a, lat: 51.2, lon: 7.1 } : a));
    rerender(<AdsbMap aircraft={moved} {...props} />);
    // Two trails (one per aircraft) and the receiver circle.
    expect(container.querySelectorAll("path.leaflet-interactive").length).toBeGreaterThanOrEqual(1);
    expect(container.querySelectorAll(".leaflet-overlay-pane path")).toHaveLength(3);
  });
});
