// Tiny example function used by the demo scenario in docs/POC.md.
// An issue like "add tests for parseAmount" triggers the test_writer agent.
//
// Real production code would obviously be more careful — this is intentionally
// minimal so the agent has something concrete to write tests for.

export function parseAmount(input: string): number {
  const cleaned = input.trim().replace(/[\s,]/g, "");
  const value = Number(cleaned);
  if (!Number.isFinite(value)) {
    throw new Error(`parseAmount: invalid input "${input}"`);
  }
  return value;
}
