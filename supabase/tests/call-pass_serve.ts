// Serves the call-pass handler with plain Deno, outside the Edge Runtime, so call-pass.sh controls
// its whole environment: run it under `env -i` to check that the function works without
// SUPABASE_ANON_KEY, or without the LiveKit secrets. Usage: deno run ... call-pass_serve.ts PORT

import { guarded, makeHandler } from "../functions/call-pass/handler.ts";

const port = Number(Deno.args[0]);
if (!Number.isInteger(port) || port <= 0) throw new Error("usage: call-pass_serve.ts PORT");

Deno.serve(
  { hostname: "127.0.0.1", port, onListen: () => {} },
  guarded(makeHandler((name) => Deno.env.get(name))),
);
