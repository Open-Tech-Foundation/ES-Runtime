import { expect, test } from "runtime:test";
import { render } from "@opentf/web-test";
import Home from "../app/page.jsx";

test("the home page counts clicks", () => {
  const { getByRole } = render(Home);
  const button = getByRole("button", { name: /count 0/i });
  button.click();
  expect(button.textContent).toMatch(/count 1/i);
});
