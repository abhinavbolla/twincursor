// call-pass: POST { "call_id": "<uuid>" } with the user's access token, get back a LiveKit token
// for that call's room, if the database says this user may join (public.pass_claims).
//
// Environment: SUPABASE_URL (set by Supabase), and LIVEKIT_URL, LIVEKIT_API_KEY and
// LIVEKIT_API_SECRET (set as function secrets). The API secret never leaves this function. The
// calls to Auth and the database use the `apikey` the app sends; SUPABASE_ANON_KEY is used only
// when a request has none (see handler.ts).

import { guarded, makeHandler } from "./handler.ts";

Deno.serve(guarded(makeHandler((name) => Deno.env.get(name))));
