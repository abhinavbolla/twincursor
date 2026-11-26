// A small menu that drops from a button: arrow keys move, Escape or a click elsewhere closes it
// and gives focus back to the button.

import { useEffect, useRef, type KeyboardEvent, type ReactNode, type RefObject } from "react";

export interface MenuItem {
  label: string;
  run: () => void;
  /** A hairline above this item. */
  separated?: boolean;
}

interface Props {
  id: string;
  label: string;
  items: MenuItem[];
  close: () => void;
  /** The button that opened it, to give focus back to. */
  trigger: RefObject<HTMLButtonElement | null>;
  header?: ReactNode;
}

export function Menu({ id, label, items, close, trigger, header }: Props) {
  const root = useRef<HTMLDivElement>(null);
  const closeRef = useRef(close);
  closeRef.current = close;

  useEffect(() => {
    root.current?.querySelector<HTMLButtonElement>("[role=menuitem]")?.focus();
    const away = (e: PointerEvent) => {
      const target = e.target as Node;
      if (!root.current?.contains(target) && !trigger.current?.contains(target)) {
        closeRef.current();
      }
    };
    document.addEventListener("pointerdown", away);
    return () => document.removeEventListener("pointerdown", away);
  }, [trigger]);

  const onKeyDown = (e: KeyboardEvent) => {
    const buttons = Array.from(root.current?.querySelectorAll<HTMLButtonElement>("[role=menuitem]") ?? []);
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
      trigger.current?.focus();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      buttons[(at + step + buttons.length) % buttons.length]?.focus();
    } else if (e.key === "Tab") {
      close();
    }
  };

  return (
    <div className="menu" id={id} ref={root} role="menu" aria-label={label} onKeyDown={onKeyDown}>
      {header}
      {items.map((item) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          className={`menu-item${item.separated === true ? " separated" : ""}`}
          onClick={() => {
            close();
            item.run();
          }}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}
