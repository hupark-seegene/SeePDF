/**
 * 도움말 › 단축키: the keymap of UI_SPEC §13 as a read-only sheet, in the platform's notation
 * (⌘⇧S on macOS, Ctrl+Shift+S on Windows). Built from `KEYMAP`, so it can never disagree with
 * what the keys actually do.
 */
import { Dialog } from "./Dialog";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { KEYMAP, chordsFor, formatChord, type KeyBinding } from "../keys/keymap";

const GROUPS: { id: KeyBinding["group"]; labelKey: string }[] = [
  { id: "file", labelKey: "menu.file" },
  { id: "edit", labelKey: "menu.edit" },
  { id: "view", labelKey: "menu.view" },
  { id: "go", labelKey: "menu.go" },
  { id: "mode", labelKey: "shortcuts.group.mode" },
  { id: "tool", labelKey: "menu.tools" },
  { id: "pages", labelKey: "pages.title" },
];

export function ShortcutsDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const os = useAppStore((s) => s.os);
  return (
    <Dialog titleKey="menu.help.shortcuts" size="lg" onClose={onClose} cancelKey="common.close">
      {GROUPS.map((group) => (
        <section key={group.id} className="shortcut-group" aria-label={t(group.labelKey)}>
          <h3 className="text-sm shortcut-heading">{t(group.labelKey)}</h3>
          <dl className="shortcut-list">
            {KEYMAP.filter((b) => b.group === group.id).map((b) => (
              <div className="shortcut-row" key={b.id}>
                <dt className="text-sm">{t(b.labelKey)}</dt>
                <dd className="mono text-sm dim">{chordsFor(b, os).map((c) => formatChord(c, os)).join(", ")}</dd>
              </div>
            ))}
          </dl>
        </section>
      ))}
    </Dialog>
  );
}
