export function StatusBar({ text, right }: { text: string; right: string }) {
  return (
    <footer className="flex h-6 shrink-0 items-center justify-between border-t border-line bg-surface px-6 text-caption text-ink-3">
      <span className="truncate">{text}</span>
      <span className="shrink-0 tabular-nums">{right}</span>
    </footer>
  );
}
