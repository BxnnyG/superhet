import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AdsbMap, TILE_ATTRIBUTION, TILE_URL } from "./AdsbMap";
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
};

describe("AdsbMap", () => {
  it("uses OpenStreetMap tiles with the attribution the tile policy asks for", () => {
    expect(TILE_URL).toBe("https://tile.openstreetmap.org/{z}/{x}/{y}.png");
    const { container } = render(<AdsbMap aircraft={picture()} {...props} />);
    const attribution = container.querySelector(".leaflet-control-attribution");
    expect(attribution).toHaveTextContent("OpenStreetMap");
    expect(attribution?.querySelector('a[href*="openstreetmap"]')).toHaveAttribute(
      "href",
      "https://www.openstreetmap.org/copyright",
    );
    expect(TILE_ATTRIBUTION).toContain("contributors");
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
