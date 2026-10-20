import type { ButtonHTMLAttributes, ReactNode, Ref } from "react";
import { BUTTON_VARIANT, type ButtonVariant } from "./Button";
import { cn } from "./cn";
import { Tooltip } from "./Tooltip";

const SIZE = { sm: "size-7", md: "size-8", lg: "size-10" } as const;

interface Props extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "type" | "aria-label"> {
  /** Accessible name; also shown as tooltip. */
  label: string;
  /** Tooltip text when it should differ from the label (e.g. a shortcut hint). */
  tip?: string;
  variant?: ButtonVariant;
  size?: keyof typeof SIZE;
  /** Toggle state (aria-pressed) for on/off buttons. */
  pressed?: boolean;
  tooltipSide?: "top" | "bottom";
  tooltipAlign?: "center" | "end";
  children: ReactNode;
  ref?: Ref<HTMLButtonElement>;
}

export function IconButton({
  label,
  tip,
  variant = "ghost",
  size = "md",
  pressed,
  tooltipSide,
  tooltipAlign,
  className,
  children,
  ...rest
}: Props) {
  return (
    <Tooltip text={tip ?? label} side={tooltipSide} align={tooltipAlign}>
      <button
        type="button"
        aria-label={label}
        aria-pressed={pressed}
        className={cn(
          "focus-ring inline-flex shrink-0 items-center justify-center rounded-md",
          "transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-45",
          BUTTON_VARIANT[variant],
          pressed && "bg-accent-soft text-accent hover:bg-accent-soft",
          SIZE[size],
          className,
        )}
        {...rest}
      >
        {children}
      </button>
    </Tooltip>
  );
}
