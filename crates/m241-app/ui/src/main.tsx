import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import App from "./App";
import "maplibre-gl/dist/maplibre-gl.css";
import "./lib/maplibre";
import "./styles.css";

const container = document.getElementById("root");
if (container === null) {
  throw new Error("index.html has no #root element");
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
