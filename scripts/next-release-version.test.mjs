import assert from "node:assert/strict";
import { test } from "node:test";
import { nextReleaseVersion } from "./next-release-version.mjs";

test("recovers from a published version whose release commit failed", () => {
  assert.equal(
    nextReleaseVersion("0.0.2", ["0.0.1", "0.0.2", "0.0.3"]),
    "0.0.4",
  );
});

test("continues normal patches when repository and npm agree", () => {
  assert.equal(nextReleaseVersion("0.0.4", ["0.0.3", "0.0.4"]), "0.0.5");
});

test("preserves a higher local release line", () => {
  assert.equal(nextReleaseVersion("1.0.0", ["0.0.3"]), "1.0.1");
});

test("compares numeric versions across patch, minor, and major boundaries", () => {
  assert.equal(nextReleaseVersion("0.0.2", ["0.0.9", "0.0.10"]), "0.0.11");
  assert.equal(nextReleaseVersion("0.9.9", ["0.10.0", "0.8.99"]), "0.10.1");
  assert.equal(nextReleaseVersion("0.99.99", ["1.0.0"]), "1.0.1");
});

test("includes versions published under another dist-tag", () => {
  assert.equal(nextReleaseVersion("0.0.2", ["0.0.4", "0.0.3"]), "0.0.5");
});

test("ignores prerelease versions when selecting the stable release line", () => {
  assert.equal(
    nextReleaseVersion("0.0.2", ["0.0.3", "1.0.0-alpha.1"]),
    "0.0.4",
  );
});

test("accepts npm's single-version response and an empty version list", () => {
  assert.equal(nextReleaseVersion("0.0.2", "0.0.3"), "0.0.4");
  assert.equal(nextReleaseVersion("0.0.2", []), "0.0.3");
});

test("rejects invalid local versions and registry response shapes", () => {
  assert.throws(() => nextReleaseVersion("broken", []), /stable release/);
  assert.throws(() => nextReleaseVersion("0.0.2", null), /version list/);
  assert.throws(() => nextReleaseVersion("0.0.2", [null]), /must be strings/);
});
