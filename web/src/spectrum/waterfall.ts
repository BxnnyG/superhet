// WebGL2 waterfall: a ring-buffer R8 texture holds one row per frame; the fragment shader
// unrolls the ring (newest row on top) and applies a turbo-like colormap.
//
// Rows are stored at a fixed texture width and carry the absolute span they were rendered with
// (see history.ts). A small float table (one texel per ring slot) maps the current view onto each
// row, so zoom, pan and centre retunes re-map the existing history instead of discarding it;
// screen areas a row never covered show the background.

import type { SpectrumFrame } from "./frame";
import { RowHistory, TABLE_STRIDE, resampleBins } from "./history";
import type { DisplayRange } from "./levels";
import { MAX_BINS } from "./socket";
import type { Span } from "./view";

export const HISTORY_ROWS = 512;
/** Fixed texture width; frames with other bin counts are resampled to it. */
export const TEXTURE_WIDTH = MAX_BINS;

const VERTEX = `#version 300 es
out vec2 v_uv;
void main() {
  // Fullscreen triangle; v_uv.y = 0 at the top of the canvas.
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  v_uv = vec2(p.x, 1.0 - p.y);
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}`;

const FRAGMENT = `#version 300 es
precision highp float;
precision highp sampler2D;
uniform sampler2D u_tex;
uniform sampler2D u_map; // per ring slot: (scale, offset, minDb, maxDb), see history.ts
uniform float u_head;    // index of the newest row
uniform float u_rows;
uniform float u_valid;   // rows written so far (<= u_rows)
uniform vec2 u_view;     // displayed dB window (low, high)
in vec2 v_uv;
out vec4 outColor;

const vec4 BG = vec4(0.02, 0.02, 0.03, 1.0);

// Polynomial approximation of the Turbo colormap (Anton Mikhailov, Apache-2.0).
vec3 turbo(float x) {
  x = clamp(x, 0.0, 1.0);
  const vec4 kR4 = vec4(0.13572138, 4.61539260, -42.66032258, 132.13108234);
  const vec4 kG4 = vec4(0.09140261, 2.19418839, 4.84296658, -14.18503333);
  const vec4 kB4 = vec4(0.10667330, 12.64194608, -60.58204836, 110.36276771);
  const vec2 kR2 = vec2(-152.94239396, 59.28637943);
  const vec2 kG2 = vec2(4.27729857, 2.82956604);
  const vec2 kB2 = vec2(-89.90310912, 27.34824973);
  vec4 v4 = vec4(1.0, x, x * x, x * x * x);
  vec2 v2 = v4.zw * v4.z;
  return clamp(vec3(dot(v4, kR4) + dot(v2, kR2),
                    dot(v4, kG4) + dot(v2, kG2),
                    dot(v4, kB4) + dot(v2, kB2)), 0.0, 1.0);
}

// Level of the row "age" rows behind the newest at screen x, normalised to the display window.
// Returns false where the row has no data at x (outside its span or not written yet).
bool level(float age, float x, out float t) {
  t = 0.0;
  if (age < 0.0 || age >= u_valid) return false;
  float row = u_head - age;
  row += u_rows * step(row, -0.5);
  vec4 m = texelFetch(u_map, ivec2(0, int(row + 0.5)), 0);
  if (m.x <= 0.0) return false;
  float u = x * m.x + m.y;
  if (u < 0.0 || u > 1.0) return false;
  float v = texture(u_tex, vec2(u, (row + 0.5) / u_rows)).r;
  float db = m.z + v * (m.w - m.z);
  t = (db - u_view.x) / (u_view.y - u_view.x);
  return true;
}

void main() {
  float age = v_uv.y * u_rows;
  float a = floor(age);
  float f = age - a;
  float ta;
  float tb;
  bool ha = level(a, v_uv.x, ta);
  bool hb = level(a + 1.0, v_uv.x, tb);
  if (!ha && !hb) {
    outColor = BG;
    return;
  }
  // Blend neighbouring rows for smooth vertical scrolling; each keeps its own span mapping.
  float t = ha && hb ? mix(ta, tb, f) : (ha ? ta : tb);
  outColor = vec4(turbo(t), 1.0);
}`;

interface GlState {
  program: WebGLProgram;
  vao: WebGLVertexArrayObject;
  texture: WebGLTexture;
  mapTexture: WebGLTexture;
  uHead: WebGLUniformLocation | null;
  uRows: WebGLUniformLocation | null;
  uValid: WebGLUniformLocation | null;
  uView: WebGLUniformLocation | null;
  uTex: WebGLUniformLocation | null;
  uMap: WebGLUniformLocation | null;
}

function compile(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader {
  const shader = gl.createShader(type);
  if (!shader) throw new Error("createShader failed");
  gl.shaderSource(shader, src);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    const log = gl.getShaderInfoLog(shader);
    gl.deleteShader(shader);
    throw new Error(`shader compile failed: ${log}`);
  }
  return shader;
}

export class Waterfall {
  private readonly gl: WebGL2RenderingContext;
  private state: GlState | null = null;
  private head = -1;
  private valid = 0;
  /** Span shown on screen: the one of the newest frame. */
  private view: Span | null = null;
  private readonly history = new RowHistory(HISTORY_ROWS);
  private readonly table = new Float32Array(HISTORY_ROWS * TABLE_STRIDE);
  private readonly rowBuf = new Uint8Array(TEXTURE_WIDTH);
  private raf = 0;
  private destroyed = false;

  /** `range` is read on every draw; mutate it and call `redraw()` to change the contrast. */
  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly range: DisplayRange,
  ) {
    const gl = canvas.getContext("webgl2", { antialias: false, alpha: false });
    if (!gl) throw new Error("WebGL2 is not available");
    this.gl = gl;
    canvas.addEventListener("webglcontextlost", this.onLost);
    canvas.addEventListener("webglcontextrestored", this.onRestored);
    this.state = this.init();
  }

  /** Redraw with the current display range (the texture keeps the raw values). */
  redraw(): void {
    this.schedule();
  }

  /** Set the backing store size in device pixels. */
  resize(width: number, height: number): void {
    const w = Math.max(1, Math.floor(width));
    const h = Math.max(1, Math.floor(height));
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    this.schedule();
  }

  push(frame: SpectrumFrame): void {
    const st = this.state;
    if (!st || frame.binCount === 0 || !(frame.sampleRate > 0)) return;
    const gl = this.gl;
    // The row keeps the span and dB mapping it arrived with; older rows stay and are re-mapped
    // onto the new view when drawing, whatever changed (zoom, pan, centre, sample rate).
    const span: Span = {
      startHz: frame.centerHz - frame.sampleRate / 2,
      endHz: frame.centerHz + frame.sampleRate / 2,
    };
    this.head = (this.head + 1) % HISTORY_ROWS;
    this.valid = Math.min(this.valid + 1, HISTORY_ROWS);
    this.history.set(this.head, span, frame.minDb, frame.maxDb);
    this.view = span;
    resampleBins(frame.bins, this.rowBuf);
    gl.bindTexture(gl.TEXTURE_2D, st.texture);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texSubImage2D(
      gl.TEXTURE_2D,
      0,
      0,
      this.head,
      TEXTURE_WIDTH,
      1,
      gl.RED,
      gl.UNSIGNED_BYTE,
      this.rowBuf,
    );
    this.schedule();
  }

  destroy(): void {
    this.destroyed = true;
    cancelAnimationFrame(this.raf);
    this.canvas.removeEventListener("webglcontextlost", this.onLost);
    this.canvas.removeEventListener("webglcontextrestored", this.onRestored);
    if (this.state && !this.gl.isContextLost()) {
      this.gl.deleteTexture(this.state.texture);
      this.gl.deleteTexture(this.state.mapTexture);
      this.gl.deleteProgram(this.state.program);
      this.gl.deleteVertexArray(this.state.vao);
    }
    this.state = null;
  }

  private readonly onLost = (ev: Event): void => {
    ev.preventDefault();
    cancelAnimationFrame(this.raf);
    this.raf = 0;
    this.state = null;
  };

  private readonly onRestored = (): void => {
    if (this.destroyed) return;
    this.state = this.init();
    this.schedule();
  };

  private init(): GlState {
    const gl = this.gl;
    const program = gl.createProgram();
    const vs = compile(gl, gl.VERTEX_SHADER, VERTEX);
    const fs = compile(gl, gl.FRAGMENT_SHADER, FRAGMENT);
    gl.attachShader(program, vs);
    gl.attachShader(program, fs);
    gl.linkProgram(program);
    gl.deleteShader(vs);
    gl.deleteShader(fs);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      throw new Error(`program link failed: ${gl.getProgramInfoLog(program)}`);
    }
    const vao = gl.createVertexArray();

    const texture = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.R8,
      TEXTURE_WIDTH,
      HISTORY_ROWS,
      0,
      gl.RED,
      gl.UNSIGNED_BYTE,
      null,
    );
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);

    // Per-slot mapping table; float textures are only fetched (texelFetch), never filtered.
    const mapTexture = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, mapTexture);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA32F, 1, HISTORY_ROWS, 0, gl.RGBA, gl.FLOAT, null);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);

    // Context state was reset (or is fresh): rows must be re-uploaded.
    this.head = -1;
    this.valid = 0;
    this.history.clear();
    return {
      program,
      vao,
      texture,
      mapTexture,
      uHead: gl.getUniformLocation(program, "u_head"),
      uRows: gl.getUniformLocation(program, "u_rows"),
      uValid: gl.getUniformLocation(program, "u_valid"),
      uView: gl.getUniformLocation(program, "u_view"),
      uTex: gl.getUniformLocation(program, "u_tex"),
      uMap: gl.getUniformLocation(program, "u_map"),
    };
  }

  private schedule(): void {
    if (this.raf || this.destroyed || !this.state) return;
    this.raf = requestAnimationFrame(() => {
      this.raf = 0;
      this.draw();
    });
  }

  private draw(): void {
    const st = this.state;
    const gl = this.gl;
    if (!st || gl.isContextLost()) return;
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.clearColor(0.02, 0.02, 0.03, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
    if (this.valid === 0 || !this.view) return;
    this.history.fillTable(this.table, this.view);
    gl.useProgram(st.program);
    gl.bindVertexArray(st.vao);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, st.mapTexture);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, 1, HISTORY_ROWS, gl.RGBA, gl.FLOAT, this.table);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, st.texture);
    gl.uniform1i(st.uTex, 0);
    gl.uniform1i(st.uMap, 1);
    gl.uniform1f(st.uHead, this.head);
    gl.uniform1f(st.uRows, HISTORY_ROWS);
    gl.uniform1f(st.uValid, this.valid);
    gl.uniform2f(st.uView, this.range.lowDb, this.range.highDb);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
}
