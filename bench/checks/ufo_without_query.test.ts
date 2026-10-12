// Copied into the run's copy by the ufo-add-without-query check; the model never sees it.
import { expect, test } from "vitest";
import { withoutQuery } from "../src";

test("withoutQuery drops the query and keeps the rest", () => {
  expect(withoutQuery("/a?b=1#c")).toBe("/a#c");
  expect(withoutQuery("https://x.dev/p?q=1")).toBe("https://x.dev/p");
  expect(withoutQuery("/plain")).toBe("/plain");
});
