import { render } from "preact";
import { App } from "./app";
import { theme } from "./store";
import "./styles.css";

document.documentElement.dataset.theme = theme.value;
render(<App />, document.getElementById("app")!);

// The window starts hidden (no white flash) and is shown after the first paint.
if ("__TAURI_INTERNALS__" in window) {
  requestAnimationFrame(() => {
    import("@tauri-apps/api/window").then((m) => m.getCurrentWindow().show());
  });
}
