import { useEffect, useState, type ReactNode } from "react";

interface Props {
  children: ReactNode;
  /** Shown after the first click; a second click confirms. */
  confirmLabel: ReactNode;
  onConfirm: () => void;
  className?: string;
  disabled?: boolean;
  title?: string;
}

/** A button that asks "are you sure?" in place instead of opening a dialog. */
export function ConfirmButton({ children, confirmLabel, onConfirm, className = "btn sm", disabled, title }: Props) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const t = setTimeout(() => setArmed(false), 4000);
    return () => clearTimeout(t);
  }, [armed]);
  return (
    <button
      type="button"
      className={`${className}${armed ? " danger" : ""}`}
      disabled={disabled}
      title={title}
      onClick={(e) => {
        e.stopPropagation();
        if (!armed) return setArmed(true);
        setArmed(false);
        onConfirm();
      }}
      onBlur={() => setArmed(false)}
    >
      {armed ? confirmLabel : children}
    </button>
  );
}
