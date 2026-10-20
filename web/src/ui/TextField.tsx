import { useId, useState, type InputHTMLAttributes, type ReactNode, type Ref } from "react";
import { cn } from "./cn";
import { IconButton } from "./IconButton";
import { EyeIcon, EyeOffIcon } from "./icons";

interface Props extends Omit<InputHTMLAttributes<HTMLInputElement>, "id" | "className"> {
  label: string;
  /** Error text; marks the field invalid (aria-invalid, aria-describedby). */
  error?: string | null;
  /** Help text below the field. */
  hint?: ReactNode;
  className?: string;
  ref?: Ref<HTMLInputElement>;
  /** Trailing control rendered inside the field border (e.g. the show/hide button). */
  trailing?: ReactNode;
}

/** Labelled single-line text input with optional hint and error. */
export function TextField({
  label,
  error,
  hint,
  className,
  trailing,
  type = "text",
  ...rest
}: Props) {
  const id = useId();
  const hintId = `${id}-hint`;
  const errId = `${id}-err`;
  const describedBy = [hint ? hintId : null, error ? errId : null].filter(Boolean).join(" ");
  return (
    <div className={cn("flex min-w-0 flex-col gap-1", className)}>
      <label htmlFor={id} className="text-xs font-medium text-fg-muted">
        {label}
      </label>
      <div
        className={cn(
          "flex h-10 items-center rounded-md border bg-surface-1 transition-colors duration-150",
          "focus-within:border-accent focus-within:ring-2 focus-within:ring-accent/30",
          error ? "border-danger" : "border-border-strong",
          rest.disabled && "opacity-45",
        )}
      >
        <input
          id={id}
          type={type}
          aria-invalid={error ? true : undefined}
          aria-describedby={describedBy || undefined}
          className="h-full min-w-0 flex-1 bg-transparent px-3 text-sm text-fg outline-none placeholder:text-fg-subtle"
          {...rest}
        />
        {trailing && <div className="pr-1">{trailing}</div>}
      </div>
      {hint && (
        <p id={hintId} className="text-xs text-fg-subtle">
          {hint}
        </p>
      )}
      {error && (
        <p id={errId} className="text-xs text-danger">
          {error}
        </p>
      )}
    </div>
  );
}

/** TextField for secrets with a show/hide toggle. */
export function PasswordField(props: Omit<Props, "type" | "trailing">) {
  const [shown, setShown] = useState(false);
  return (
    <TextField
      {...props}
      type={shown ? "text" : "password"}
      spellCheck={false}
      autoCapitalize="off"
      trailing={
        <IconButton
          label={shown ? "Hide password" : "Show password"}
          size="sm"
          pressed={shown}
          tabIndex={-1}
          tooltipAlign="end"
          onClick={() => setShown((s) => !s)}
        >
          {shown ? <EyeOffIcon /> : <EyeIcon />}
        </IconButton>
      }
    />
  );
}
