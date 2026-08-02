import assert from "node:assert/strict";
import test from "node:test";
import { terrainApi } from "../app/terrain/api.ts";

// terrainApi wraps global fetch, so each test swaps in a stub and restores
// the original afterward. listSetups exercises the shared requestJson path.

const realFetch = globalThis.fetch;

function withFetch(stub, run) {
  globalThis.fetch = stub;
  return run().finally(() => {
    globalThis.fetch = realFetch;
  });
}

test("surfaces the error field from a failed JSON response", () =>
  withFetch(
    async () =>
      new Response(JSON.stringify({ error: "Setup not found." }), {
        status: 404,
      }),
    async () => {
      await assert.rejects(terrainApi.listSetups(), /Setup not found\./);
    },
  ));

test("surfaces the message field when there is no error field", () =>
  withFetch(
    async () =>
      new Response(JSON.stringify({ message: "Too many jobs." }), {
        status: 429,
      }),
    async () => {
      await assert.rejects(terrainApi.listSetups(), /Too many jobs\./);
    },
  ));

test("falls back to the status for non-JSON error bodies", () =>
  withFetch(
    async () => new Response("<html>Bad gateway</html>", { status: 502 }),
    async () => {
      await assert.rejects(
        terrainApi.listSetups(),
        /TopoSaic service returned 502\./,
      );
    },
  ));

test("keeps the friendly message for a 200 with a non-JSON body", () =>
  withFetch(
    async () => new Response("not json at all", { status: 200 }),
    async () => {
      await assert.rejects(terrainApi.listSetups(), (error) => {
        assert.ok(error instanceof Error);
        assert.ok(!(error instanceof SyntaxError));
        assert.match(error.message, /TopoSaic service returned 200/);
        return true;
      });
    },
  ));

test("rethrows aborts instead of wrapping them", () =>
  withFetch(
    async () => ({
      ok: true,
      status: 200,
      json: async () => {
        throw new DOMException("Aborted", "AbortError");
      },
    }),
    async () => {
      await assert.rejects(terrainApi.listSetups(), (error) => {
        assert.ok(error instanceof DOMException);
        assert.equal(error.name, "AbortError");
        return true;
      });
    },
  ));

test("preview streams stage progress before returning the model", () => {
  const progress = [];
  const preview = { width: 2, height: 2, values: [0, 0.3, 0.7, 1] };
  const body = [
    JSON.stringify({
      type: "progress",
      stage: "surface",
      label: "Loading roads and paths",
      progress: 53,
    }),
    JSON.stringify({ type: "complete", preview }),
    "",
  ].join("\n");
  return withFetch(
    async (_url, init) => {
      assert.equal(init?.headers.accept, "application/x-ndjson");
      assert.equal(init?.headers["x-toposaic-preview-detail"], "high");
      assert.equal(init?.headers["x-toposaic-preview-id"], "preview-17");
      assert.equal(init?.headers["x-toposaic-preview-tile-row"], "2");
      assert.equal(init?.headers["x-toposaic-preview-tile-column"], "3");
      return new Response(body, {
        status: 200,
        headers: { "content-type": "application/x-ndjson" },
      });
    },
    async () => {
      const result = await terrainApi.preview(
        {},
        undefined,
        (event) => {
          progress.push(event);
        },
        "high",
        "preview-17",
        { row: 2, column: 3 },
      );
      assert.deepEqual(result, preview);
      assert.deepEqual(progress, [
        {
          stage: "surface",
          label: "Loading roads and paths",
          progress: 53,
        },
      ]);
    },
  );
});

test("cancels one named preview without stopping its replacement", () => {
  let captured;
  return withFetch(
    async (url, init) => {
      captured = { url: String(url), method: init?.method };
      return new Response(JSON.stringify({ canceled: true }), { status: 200 });
    },
    async () => {
      assert.deepEqual(await terrainApi.cancelPreview("preview/17"), {
        canceled: true,
      });
      assert.match(captured.url, /\/api\/preview\/preview%2F17$/);
      assert.equal(captured.method, "DELETE");
    },
  );
});

test("preview reports a server-side replacement as an abort", () =>
  withFetch(
    async () =>
      new Response('{"type":"canceled"}\n', {
        status: 200,
        headers: { "content-type": "application/x-ndjson" },
      }),
    async () => {
      await assert.rejects(terrainApi.preview({}), (error) => {
        assert.ok(error instanceof DOMException);
        assert.equal(error.name, "AbortError");
        return true;
      });
    },
  ));

test("saveSetup reports a 201 as created", () =>
  withFetch(
    async () =>
      new Response(JSON.stringify({ id: "s1", name: "Ridge" }), {
        status: 201,
      }),
    async () => {
      const result = await terrainApi.saveSetup("Ridge", {});
      assert.equal(result.created, true);
      assert.equal(result.setup.name, "Ridge");
    },
  ));

test("saveSetup reports a 200 overwrite as not created", () =>
  withFetch(
    async () =>
      new Response(JSON.stringify({ id: "s1", name: "Ridge" }), {
        status: 200,
      }),
    async () => {
      const result = await terrainApi.saveSetup("Ridge", {});
      assert.equal(result.created, false);
      assert.equal(result.setup.id, "s1");
    },
  ));

test("saveSetup still surfaces error details", () =>
  withFetch(
    async () =>
      new Response(JSON.stringify({ error: "Name too long." }), {
        status: 400,
      }),
    async () => {
      await assert.rejects(
        terrainApi.saveSetup("Ridge", {}),
        /Name too long\./,
      );
    },
  ));

test("clearCache posts the age and parses the removal counts", () => {
  let captured;
  return withFetch(
    async (url, init) => {
      captured = { url: String(url), body: init?.body };
      return new Response(
        JSON.stringify({ removed_bytes: 4096, removed_entries: 3 }),
        { status: 200 },
      );
    },
    async () => {
      const result = await terrainApi.clearCache(30);
      assert.deepEqual(result, { removed_bytes: 4096, removed_entries: 3 });
      assert.match(captured.url, /\/api\/cache\/clear$/);
      assert.deepEqual(JSON.parse(captured.body), { older_than_days: 30 });
    },
  );
});

test("clearCache sends an explicit null for a full clear", () => {
  let captured;
  return withFetch(
    async (_url, init) => {
      captured = init?.body;
      return new Response(
        JSON.stringify({ removed_bytes: 0, removed_entries: 0 }),
        { status: 200 },
      );
    },
    async () => {
      await terrainApi.clearCache(null);
      assert.deepEqual(JSON.parse(captured), { older_than_days: null });
    },
  );
});

test("deleteSetup surfaces the server's error detail", () =>
  withFetch(
    async () =>
      new Response(JSON.stringify({ error: "Setup is read-only." }), {
        status: 409,
      }),
    async () => {
      await assert.rejects(
        terrainApi.deleteSetup("abc"),
        /Setup is read-only\./,
      );
    },
  ));

test("deleteSetup falls back to the status without a detail", () =>
  withFetch(
    async () => new Response("plain text", { status: 500 }),
    async () => {
      await assert.rejects(
        terrainApi.deleteSetup("abc"),
        /TopoSaic service returned 500\./,
      );
    },
  ));

test("deleteSetup passes its abort signal through to fetch", () => {
  let received;
  return withFetch(
    async (_url, init) => {
      received = init?.signal;
      return new Response(null, { status: 204 });
    },
    async () => {
      const controller = new AbortController();
      await terrainApi.deleteSetup("abc", controller.signal);
      assert.equal(received, controller.signal);
    },
  );
});
