import { SpectrumView } from "./spectrum/SpectrumView";

export function App() {
  return (
    <main className="flex h-screen flex-col gap-3 bg-neutral-950 p-3 text-neutral-100">
      <h1 className="text-lg font-semibold tracking-tight">Superhet – pre-alpha</h1>
      <div className="min-h-0 flex-1">
        <SpectrumView />
      </div>
    </main>
  );
}
