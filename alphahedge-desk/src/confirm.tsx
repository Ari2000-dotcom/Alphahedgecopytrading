import { signal } from "@preact/signals";
import { useEffect, useRef } from "preact/hooks";

export interface ConfirmOptions {
  title: string;
  /** One paragraph per entry. */
  body: string[];
  confirmLabel: string;
  cancelLabel?: string;
  /** Red confirm button, for things that cannot be undone. */
  danger?: boolean;
  onConfirm: () => void;
}

const current = signal<ConfirmOptions | null>(null);
let returnTo: HTMLElement | null = null;

/** Ask the user to confirm. Nothing happens unless they press the confirm button. */
export function askConfirm(o: ConfirmOptions) {
  returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  current.value = o;
}

function close() {
  current.value = null;
  returnTo?.isConnected && returnTo.focus();
  returnTo = null;
}

/** Mount once. Renders the open question, if any. */
export function ConfirmHost() {
  const o = current.value;
  const box = useRef<HTMLDivElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);

  // The safe choice has focus, so a stray Enter cancels instead of deleting.
  useEffect(() => {
    if (o) cancel.current?.focus();
  }, [o]);

  if (!o) return null;

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "Tab") {
      // Keep Tab inside the dialog.
      const items = Array.from(box.current?.querySelectorAll<HTMLElement>("button") ?? []);
      if (!items.length) return;
      const first = items[0], last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
    }
  }

  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) close(); }}>
      <div ref={box} class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="cf-title" aria-describedby="cf-body" onKeyDown={onKey}>
        <h2 id="cf-title">{o.title}</h2>
        <div id="cf-body" class="dbody">{o.body.map((p) => <p key={p}>{p}</p>)}</div>
        <div class="dact">
          <button ref={cancel} class="btn" onClick={close}>{o.cancelLabel ?? "Cancel"}</button>
          <button
            class={"btn " + (o.danger ? "danger" : "primary")}
            onClick={() => { const run = o.onConfirm; close(); run(); }}
          >{o.confirmLabel}</button>
        </div>
      </div>
    </div>
  );
}
