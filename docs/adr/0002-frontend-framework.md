# ADR-0002: Frontend framework

**Status:** accepted (2026-10-20)

## Context

The UI should look and feel like current infra products (shadcn/Tailwind aesthetics, dark default, command palette, shortcuts) and host third-party UI plugins later. Performance-critical parts (WebGL2 waterfall at 25+ fps, AudioWorklet audio) must not depend on the framework's render cycle anyway.

## Options

1. **SvelteKit** (static adapter) + shadcn-svelte: small bundles, little boilerplate, good reactivity.
2. **React + Vite (SPA)** + Tailwind + shadcn/ui: largest ecosystem; shadcn/ui, Radix, cmdk, TanStack Query/Table are the reference implementations; most potential plugin authors know React.

No SSR/routing-heavy needs exist (single-user SPA served by the Rust server), so SvelteKit's main strengths are not needed.

## Decision

**React + Vite + TypeScript (strict) + Tailwind CSS v4 + shadcn/ui-style components (Radix primitives), pnpm.** State: a small store (Zustand) for UI state; server state via TanStack Query; WebSocket stream data bypasses React and goes straight into the WebGL/Audio modules. Waterfall: WebGL2 first, WebGPU as optional later backend.

## Consequences

- UI plugin API can be offered as React components *and* framework-agnostic mount functions (`mount(el, api)`), so plugin authors are not forced into React.
- Bundle size is larger than with Svelte; acceptable for a self-hosted single-user app.
