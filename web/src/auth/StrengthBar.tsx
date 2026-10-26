import { cn } from "../ui";
import { passwordStrength } from "./password";

const BAR = ["bg-border-strong", "bg-danger", "bg-warn", "bg-success", "bg-success"] as const;

/** Four-segment password strength hint; renders nothing for an empty password. */
export function StrengthBar({ password }: { password: string }) {
  if (!password) return null;
  const strength = passwordStrength(password);
  return (
    <div className="flex items-center gap-2" data-testid="strength">
      <div className="flex flex-1 gap-1" aria-hidden="true">
        {[1, 2, 3, 4].map((i) => (
          <span
            key={i}
            className={cn(
              "h-1 flex-1 rounded-full",
              strength.score >= i ? BAR[strength.score] : "bg-border-strong",
            )}
          />
        ))}
      </div>
      <span className="w-16 text-right text-xs text-fg-muted">Strength: {strength.label}</span>
    </div>
  );
}
