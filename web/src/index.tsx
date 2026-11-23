/* @refresh reload */
import { render } from "solid-js/web";
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./app.css";
import App from "./App";
import { store } from "./lib/data";

void store.init();
render(() => <App />, document.getElementById("root")!);
