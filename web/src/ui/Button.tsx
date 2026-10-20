import type { ButtonHTMLAttributes, ReactNode, Ref } from "react";
import { cn } from "./cn";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
export type ButtonSize = "sm" | "md" | "lg";

export const BUTTON_VARIANT: Record<ButtonVariant, string> = {
  primary: "bg-accent text-accent-fg hover:bg-accent-hover border border-transparent",
  secondary: "bg-surface-1 text-fg border border-border-strong hover:bg-surface-2",
  ghost: "bg-transparent text-fg-muted border border-transparent hover:bg-surface-2 hover:text-fg",
  danger: "bg-danger-soft text-danger border border-transparent hover:brightness-95",
};

export const BUTTON_SIZE: Record<ButtonSize, string> = {
  sm: "h-7 gap-1.5 px-2.5 text-xs",
  md: "h-8 gap-2 px-3 text-sm",
  lg: "h-10 gap-2 px-4 text-sm",
};

interface Props extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "type"> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** Icon rendered before the label. */
  icon?: ReactNode;
  type?: "button" | "submit";
  ref?: Ref<HTMLButtonElement>;
}

export function Button({
  variant = "secondary",
  size = "md",
  icon,
  type = "button",
  className,
  children,
  ...rest
}: Props) {
  return (
    <button
      type={type}
      className={cn(
        "focus-ring inline-flex shrink-0 items-center justify-center rounded-md font-medium",
        "transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-45",
        BUTTON_VARIANT[variant],
        BUTTON_SIZE[size],
        className,
      )}
      {...rest}
    >
      {icon}
      {children}
    </button>
  );
}
