import testConfig from "./vitest.config.js";

export default {
  ...testConfig,
  test: {
    ...testConfig.test,
    include: ["benchmarks/**/*.benchmark.ts"],
    fileParallelism: false,
  },
};
