import { expect, test } from "runtime:test";
import { render } from "@opentf/web-test";
import { Counter } from "../index.js";

test("renders the initial count", () => {
  const { getByRole, unmount } = render(Counter, { initial: 3 });
  expect(getByRole("button", { name: "Count 3" })).toBeTruthy();
  unmount();
});

test("clicking increments the count", () => {
  const { getByRole, unmount } = render(Counter);
  const button = getByRole("button", { name: "Count 0" });
  button.click();
  button.click();
  expect(button.textContent).toBe("Count 2");
  unmount();
});

test("unmounting removes the component", () => {
  const { container, getByRole, unmount } = render(Counter);
  expect(getByRole("button")).toBeTruthy();
  unmount();
  expect(container.isConnected).toBe(false);
  expect(document.querySelector("[data-testid='counter']")).toBe(null);
});
