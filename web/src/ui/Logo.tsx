import { cn } from "./cn";

/** Superhet mark (rounded square with a wave) plus wordmark. */
export function Logo({ size = 22, className }: { size?: number; className?: string }) {
  return (
    <div className={cn("flex items-center gap-2", className)}>
      <svg
        width={size}
        height={size}
        viewBox="0 0 24 24"
        aria-hidden="true"
        className="text-accent"
      >
        <rect width="24" height="24" rx="6" fill="currentColor" />
        <path
          d="M4 14c2.5 0 2.5-4 5-4s2.5 6 5 6 2.5-8 6-8"
          fill="none"
          stroke="var(--accent-fg)"
          strokeWidth="2"
          strokeLinecap="round"
        />
      </svg>
      <span className="font-semibold tracking-tight" style={{ fontSize: Math.round(size * 0.64) }}>
        Superhet
      </span>
    </div>
  );
}
