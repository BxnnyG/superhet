// All trails on one canvas under the aircraft symbols: one draw call per segment instead of
// thousands of Leaflet path objects, which keeps 200+ aircraft with long trails cheap. The canvas
// covers the map view and is redrawn when the picture or the view changes (at most once per
// frame).

import L from "leaflet";
import { altColor } from "./model";
import { trailAlpha, type TrailSegment } from "./trails";

export interface TrailItem {
  segments: readonly TrailSegment[];
  /** The aircraft's own fade (1 fresh .. 0.25 stale). */
  opacity: number;
  selected: boolean;
}

const PANE = "adsb-trails";

export class TrailCanvas {
  private readonly canvas: HTMLCanvasElement;
  private items: readonly TrailItem[] = [];
  private frame = 0;
  private readonly redrawSoon = (): void => {
    if (this.frame !== 0) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.draw();
    });
  };

  constructor(private readonly map: L.Map) {
    const pane = map.getPane(PANE) ?? map.createPane(PANE);
    pane.style.zIndex = "450";
    pane.style.pointerEvents = "none";
    this.canvas = L.DomUtil.create("canvas", "adsb-trail-canvas", pane);
    this.canvas.setAttribute("aria-hidden", "true");
    map.on("move zoom resize viewreset", this.redrawSoon);
  }

  set(items: readonly TrailItem[]): void {
    this.items = items;
    this.redrawSoon();
  }

  destroy(): void {
    this.map.off("move zoom resize viewreset", this.redrawSoon);
    if (this.frame !== 0) cancelAnimationFrame(this.frame);
    this.canvas.remove();
  }

  private draw(): void {
    const size = this.map.getSize();
    const dpr = window.devicePixelRatio || 1;
    const { canvas } = this;
    // The canvas stays at the top left of what is visible while the map pane moves.
    L.DomUtil.setPosition(canvas, this.map.containerPointToLayerPoint([0, 0]));
    if (canvas.width !== Math.round(size.x * dpr) || canvas.height !== Math.round(size.y * dpr)) {
      canvas.width = Math.round(size.x * dpr);
      canvas.height = Math.round(size.y * dpr);
      canvas.style.width = `${size.x}px`;
      canvas.style.height = `${size.y}px`;
    }
    const ctx = canvas.getContext("2d");
    if (ctx === null) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, size.x, size.y);
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    const project = (p: { lat: number; lon: number }): L.Point =>
      this.map.latLngToContainerPoint([p.lat, p.lon]);
    for (const item of this.items) {
      ctx.lineWidth = item.selected ? 3 : 2;
      for (const seg of item.segments) {
        const a = project(seg.from);
        const b = project(seg.to);
        // Off screen entirely: nothing to draw.
        if (
          (a.x < 0 && b.x < 0) ||
          (a.y < 0 && b.y < 0) ||
          (a.x > size.x && b.x > size.x) ||
          (a.y > size.y && b.y > size.y)
        ) {
          continue;
        }
        ctx.globalAlpha = trailAlpha(seg.strength) * item.opacity;
        ctx.strokeStyle = altColor(seg.to.altFt, seg.to.onGround);
        ctx.beginPath();
        ctx.moveTo(a.x, a.y);
        ctx.lineTo(b.x, b.y);
        ctx.stroke();
      }
    }
    ctx.globalAlpha = 1;
  }
}
