import { expect, test } from "bun:test";
import { createEvents } from "./events.js";
import { createTree } from "./tree.js";

const events = createEvents();
const { Document } = createTree(events);

test("dispatches capture, target and bubble over a frozen ancestor path", () => {
  const document = new Document();
  const root = document.createElement("main");
  const child = document.createElement("button");
  document.appendChild(root); root.appendChild(child);
  const calls = [];
  document.addEventListener("save", () => calls.push("document-capture"), true);
  root.addEventListener("save", () => { calls.push("root-capture"); root.removeChild(child); }, true);
  child.addEventListener("save", (event) => { calls.push(`target:${event.target.localName}:${event.currentTarget.localName}`); });
  root.addEventListener("save", () => calls.push("root-bubble"));
  document.addEventListener("save", () => calls.push("document-bubble"));

  child.dispatchEvent(new events.Event("save", { bubbles: true }));
  expect(calls).toEqual(["document-capture", "root-capture", "target:button:button", "root-bubble", "document-bubble"]);
});

test("honors once, passive, signals, and immediate propagation", () => {
  const document = new Document();
  const target = document.createElement("button");
  const calls = [];
  const controller = new AbortController();
  target.addEventListener("go", () => calls.push("once"), { once: true });
  target.addEventListener("go", (event) => { event.preventDefault(); calls.push("passive"); }, { passive: true });
  target.addEventListener("go", () => calls.push("aborted"), { signal: controller.signal });
  target.addEventListener("go", (event) => { calls.push("stop"); event.stopImmediatePropagation(); });
  target.addEventListener("go", () => calls.push("never"));
  controller.abort();

  const event = new events.Event("go", { cancelable: true });
  expect(target.dispatchEvent(event)).toBe(true);
  target.dispatchEvent(new events.Event("go"));
  expect(calls).toEqual(["once", "passive", "stop", "passive", "stop"]);
  expect(event.defaultPrevented).toBe(false);
});

test("event constructors preserve their defined values", () => {
  const input = new events.InputEvent("input", { data: "x", inputType: "insertText", bubbles: true });
  const pointer = new events.PointerEvent("pointerdown", { pointerId: 3, clientX: 12, pointerType: "mouse" });
  const custom = new events.CustomEvent("ready", { detail: { ok: true } });
  expect(input.data).toBe("x");
  expect(input.bubbles).toBe(true);
  expect(pointer.pointerId).toBe(3);
  expect(pointer.clientX).toBe(12);
  expect(custom.detail).toEqual({ ok: true });
});

test("legacy events initialize before dispatch and invoke inline handlers", () => {
  const document = new Document();
  const target = document.createElement("button");
  const event = document.createEvent("Event");
  const calls = [];
  target.onclick = (received) => { calls.push(received.type); received.preventDefault(); };
  event.initEvent("click", true, true);

  expect(target.dispatchEvent(event)).toBe(false);
  expect(calls).toEqual(["click"]);
});
