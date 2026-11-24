import "./ui/style.css";
import { App } from "./ui/app";
import { webHidSupported } from "./transport/webhid";

const root = document.getElementById("app")!;
const app = new App(root);
app.mount();

// `?mock=1` (optionally `&fw=0.0.9`) opens the built-in mock device straight away.
const params = new URLSearchParams(location.search);
if (params.has("mock")) {
  const fw = (params.get("fw") ?? "0.0.18").split(".").map(Number);
  const version: [number, number, number] = fw.length === 3 && fw.every((n) => Number.isInteger(n) && n >= 0 && n < 256)
    ? [fw[0], fw[1], fw[2]]
    : [0, 0, 18];
  void app.connectMock(version);
} else if (webHidSupported()) {
  // reconnect silently to a keyboard this origin was already allowed to use
  void app.connectGranted();
}

// for poking around from the console
(window as unknown as { zent: App }).zent = app;
