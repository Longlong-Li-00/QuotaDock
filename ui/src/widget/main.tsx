import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Widget } from "./Widget.tsx";
import "../tokens.css";
import "./widget.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Widget />
  </StrictMode>,
);
