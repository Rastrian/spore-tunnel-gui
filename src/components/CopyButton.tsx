import { useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";

/**
 * Copies fixed text to the system clipboard and flashes a check mark.
 * Display feedback only — no data fetching here.
 */
export function CopyButton({
  text,
  title = "Copy",
  onCopied,
}: {
  text: string;
  title?: string;
  onCopied?: (err: unknown) => void;
}) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setCopied(false), 1500);
    } catch (err) {
      onCopied?.(err);
    }
  }

  return (
    <button
      type="button"
      onClick={copy}
      title={copied ? "Copied!" : title}
      aria-label={copied ? "Copied" : title}
      className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-md border transition-colors ${
        copied
          ? "border-accent/50 text-accent"
          : "border-line text-dim hover:text-ink"
      }`}
    >
      {copied ? <Check size={14} /> : <Copy size={14} />}
    </button>
  );
}
