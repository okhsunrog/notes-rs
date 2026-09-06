import { expect, it } from "vite-plus/test";
import { canDrawHandwriting, editorAccess } from "./handwriting-access";

it("allows drawing with a detected pen or the explicit mouse preference", () => {
  expect(canDrawHandwriting(true, false)).toBe(true);
  expect(canDrawHandwriting(false, true)).toBe(true);
  expect(canDrawHandwriting(false, false)).toBe(false);
});

it("keeps a note readable when it cannot be edited", () => {
  expect(editorAccess("owned", true)).toBe("editable");
  expect(editorAccess("pending", true)).toBe("pending");
  expect(editorAccess("owned", false)).toBe("no_input");
  expect(editorAccess("taken", true)).toBe("other_pane");
  expect(editorAccess("taken", false)).toBe("other_pane");
});
