import { Dialog, Kbd } from "../ui";
import { SHORTCUT_GROUPS } from "./shortcuts";

/** Reference of the keyboard shortcuts, opened with "?" or from the settings menu. */
export function ShortcutsDialog({ onClose }: { onClose: () => void }) {
  return (
    <Dialog title="Keyboard shortcuts" onClose={onClose}>
      <div className="flex flex-col gap-4">
        {SHORTCUT_GROUPS.map((group) => (
          <section key={group.title} aria-label={group.title} className="flex flex-col gap-1.5">
            <h3 className="text-xs font-medium tracking-wide text-fg-subtle uppercase">
              {group.title}
            </h3>
            <dl className="flex flex-col gap-1">
              {group.items.map((item) => (
                <div key={item.label} className="flex items-center justify-between gap-3 text-sm">
                  <dt className="text-fg-muted">{item.label}</dt>
                  <dd className="flex shrink-0 items-center gap-1">
                    {item.keys.map((k) => (
                      <Kbd key={k}>{k}</Kbd>
                    ))}
                  </dd>
                </div>
              ))}
            </dl>
          </section>
        ))}
        <p className="text-xs text-fg-subtle">
          Shortcuts are ignored while typing in a field or while a control has the arrow keys.
        </p>
      </div>
    </Dialog>
  );
}
