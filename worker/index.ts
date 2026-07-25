/** Cloudflare Worker entry point that serves the TopoSaic web app. */
import handler from "vinext/server/app-router-entry";

interface Env {
  ASSETS: { fetch: typeof fetch };
}

interface ExecutionContext {
  waitUntil(promise: Promise<unknown>): void;
  passThroughOnException(): void;
}

const worker = {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    return handler.fetch(request, env, ctx);
  },
};

export default worker;
