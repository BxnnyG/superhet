import type { ReactNode } from "react";
import { Logo } from "../ui";

/** Centred card on the page background, scrollable on short screens (the body never scrolls). */
export function AuthLayout({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle?: string;
  children: ReactNode;
}) {
  return (
    <div className="h-full overflow-y-auto bg-surface-0 text-fg">
      <main className="mx-auto flex min-h-full w-full max-w-md flex-col justify-center gap-6 px-4 py-8">
        <Logo size={32} className="justify-center" />
        <div className="flex flex-col gap-5 rounded-xl border border-border bg-surface-1 p-5 shadow-pop sm:p-6">
          <div className="flex flex-col gap-1">
            <h1 className="text-lg font-semibold tracking-tight">{title}</h1>
            {subtitle && <p className="text-sm text-fg-muted">{subtitle}</p>}
          </div>
          {children}
        </div>
      </main>
    </div>
  );
}
