import {defineConfig} from "vitest/config";

// Qualify product sources only; .local/.reference contain independent historical copies.
export default defineConfig({
  test: {
    include: ["{apps,packages,tests,tools}/**/*.{test,spec}.?(c|m)[jt]s?(x)"],
  },
});
