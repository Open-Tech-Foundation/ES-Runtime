import { expect, test } from "runtime:test";
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

test("inline handlers run during modern event dispatch", () => {
  const document = new Document();
  const target = document.createElement("button");
  const calls = [];
  target.onclick = (received) => { calls.push(received.type); received.preventDefault(); };

  expect(target.dispatchEvent(new events.Event("click", { bubbles: true, cancelable: true }))).toBe(false);
  expect(calls).toEqual(["click"]);
});

test("constructed targets expose standard event state and listener options", () => {
  const target = new events.EventTarget();
  const event = new events.Event("go", { cancelable: true });
  let seenEvent;
  target.addEventListener("go", { get handleEvent() { seenEvent = globalThis.event; return (received) => received.returnValue = false; } });
  target.dispatchEvent(event);

  expect(seenEvent).toBe(event);
  expect(event.srcElement).toBe(target);
  expect(event.defaultPrevented).toBe(true);
  expect(event.composedPath()).toEqual([]);
  expect(Object.getOwnPropertyDescriptor(event, "isTrusted")?.get).toBe(Object.getOwnPropertyDescriptor(new events.Event("go"), "isTrusted")?.get);
  event.initEvent("again", true, false);
  expect([event.type, event.bubbles, event.cancelable, event.defaultPrevented]).toEqual(["again", true, false, false]);

  const passive = new events.Event("passive", { cancelable: true });
  target.addEventListener("passive", (received) => received.preventDefault(), { passive: true });
  expect(target.dispatchEvent(passive)).toBe(true);
  expect(passive.defaultPrevented).toBe(false);
});

test("creates an event by modern interface name and refuses the HTML4 aliases", () => {
  const document = new Document();
  const target = document.createElement("div");
  document.appendChild(target);
  const event = events.createLegacy("Event");

  // Uninitialized until `initEvent`, so dispatching it is an error.
  expect(event.type).toBe("");
  expect(() => target.dispatchEvent(event)).toThrow("not been initialized");
  let seen = null;
  target.addEventListener("change", (received) => { seen = [received.type, received.bubbles, received.cancelable]; });
  event.initEvent("change", true, false);
  expect(target.dispatchEvent(event)).toBe(true);
  expect(seen).toEqual(["change", true, false]);

  expect(events.createLegacy("MouseEvent")).toBeInstanceOf(events.MouseEvent);
  expect(events.createLegacy("customevent")).toBeInstanceOf(events.CustomEvent);
  expect(() => events.createLegacy("HTMLEvents")).toThrow("HTML4 name");
  expect(() => events.createLegacy("UIEvents")).toThrow("new UIEvent");
  expect(() => events.createLegacy("Nonsense")).toThrow("not an event interface");
});
