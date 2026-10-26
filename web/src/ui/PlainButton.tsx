import type { ButtonHTMLAttributes, Ref } from "react";
import { cn } from "./cn";

interface Props extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "type"> {
  ref?: Ref<HTMLButtonElement>;
}

/**
 * Unstyled button with the focus ring, for composite controls built in the shell (tabs, pills,
 * tab bars) that bring their own look through `className`.
 */
export function PlainButton({ className, children, ref, ...rest }: Props) {
  return (
    <button ref={ref} type="button" className={cn("focus-ring", className)} {...rest}>
      {children}
    </button>
  );
}
