import { useId, useState, type ReactNode } from "react";
import { cn } from "./cn";
import { ChevronDownIcon } from "./icons";

interface SectionProps {
  title: string;
  defaultOpen?: boolean;
  /** Controls shown on the right of the header (they must not be nested buttons of the toggle). */
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}

/** Collapsible sidebar section: small uppercase title, chevron, content kept mounted when closed. */
export function Section({ title, defaultOpen = true, actions, children, className }: SectionProps) {
  const [open, setOpen] = useState(defaultOpen);
  const bodyId = useId();
  return (
    <section className={cn("border-b border-border", className)}>
      <div className="flex items-center gap-1 pr-3 pl-1">
        <button
          type="button"
          aria-expanded={open}
          aria-controls={bodyId}
          onClick={() => setOpen((o) => !o)}
          className={cn(
            "focus-ring flex h-10 min-w-0 flex-1 items-center gap-1.5 rounded-md px-2 text-left",
            "text-xs font-semibold tracking-wider text-fg-muted uppercase",
            "transition-colors duration-150 hover:text-fg",
          )}
        >
          <ChevronDownIcon
            size={14}
            className={cn("transition-transform duration-150", !open && "-rotate-90")}
          />
          {title}
        </button>
        {actions}
      </div>
      <div id={bodyId} hidden={!open} className="flex flex-col gap-3 px-4 pt-1 pb-4">
        {children}
      </div>
    </section>
  );
}

interface PanelProps {
  title?: string;
  children: ReactNode;
  className?: string;
}

/** Plain card surface: 1px border, 12px radius. */
export function Panel({ title, children, className }: PanelProps) {
  return (
    <div className={cn("rounded-lg border border-border bg-surface-1 p-3", className)}>
      {title && (
        <h3 className="mb-2 text-xs font-semibold tracking-wider text-fg-muted uppercase">
          {title}
        </h3>
      )}
      {children}
    </div>
  );
}
