import assert from "node:assert/strict";
import { readdir, stat } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";

const MAX_CLIENT_CHUNK_BYTES = 500_000;

test("keeps production client chunks below the build warning limit", async () => {
  const assetsDirectory = join(process.cwd(), "dist", "client", "assets");
  const chunks = (await readdir(assetsDirectory)).filter((name) =>
    name.endsWith(".js"),
  );
  const sizes = await Promise.all(
    chunks.map(async (name) => ({
      name,
      bytes: (await stat(join(assetsDirectory, name))).size,
    })),
  );
  const oversized = sizes.filter(
    ({ bytes }) => bytes > MAX_CLIENT_CHUNK_BYTES,
  );

  assert.deepEqual(
    oversized,
    [],
    `client chunks exceed ${MAX_CLIENT_CHUNK_BYTES} bytes`,
  );
});
