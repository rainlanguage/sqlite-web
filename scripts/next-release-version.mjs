import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

function stableVersionParts(version) {
  if (typeof version !== "string") {
    throw new Error("Package versions must be strings.");
  }
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)) {
    return null;
  }
  return version.split(".").map(BigInt);
}

// Release stable patches above every published stable version, including ones
// whose release commit failed or whose npm dist-tag is not `latest`.
export function nextReleaseVersion(localVersion, publishedVersions) {
  let base = stableVersionParts(localVersion);
  if (!base) {
    throw new Error("The local package must have a stable release version.");
  }
  const versions = typeof publishedVersions === "string"
    ? [publishedVersions]
    : publishedVersions;
  if (!Array.isArray(versions)) {
    throw new Error("Expected npm's published version list.");
  }
  for (const version of versions) {
    const parts = stableVersionParts(version);
    if (!parts) continue;
    for (let index = 0; index < 3; index++) {
      if (parts[index] > base[index]) {
        base = parts;
        break;
      }
      if (parts[index] < base[index]) break;
    }
  }
  return `${base[0]}.${base[1]}.${base[2] + 1n}`;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  const { version } = JSON.parse(readFileSync("pkg/package.json", "utf8"));
  const publishedVersions = JSON.parse(process.argv[2]);
  console.log(nextReleaseVersion(version, publishedVersions));
}
