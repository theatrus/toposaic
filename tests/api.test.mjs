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
