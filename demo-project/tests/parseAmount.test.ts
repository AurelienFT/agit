// Placeholder. The test_writer agent will fill this in when run against
// an issue labeled `agent:test` referencing `parseAmount`.
//
// We keep one trivial assertion so the test command doesn't fail on an
// empty suite before the agent has done its work.

import { parseAmount } from "../src/parseAmount";

test("parseAmount is exported", () => {
  expect(typeof parseAmount).toBe("function");
});
