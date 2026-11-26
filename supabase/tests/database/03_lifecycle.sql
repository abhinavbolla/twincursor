-- How calls end and come back: start_call replacing a call, leaving, the host heartbeat, a stale
-- host reviving, ringing on a stale call, and end_call.
--   d1 Hal (host)   d2 Gia (guest)   d3 Kit (knocks late)
begin;
create extension if not exists pgtap with schema extensions;
select plan(54);

insert into auth.users (id, email, raw_user_meta_data) values
  ('00000000-0000-4000-8000-0000000000d1', 'hal@example.test', '{"name": "Hal"}'),
  ('00000000-0000-4000-8000-0000000000d2', 'gia@example.test', '{"name": "Gia"}'),
  ('00000000-0000-4000-8000-0000000000d3', 'kit@example.test', '{"name": "Kit"}');

-- Call 1: Hal hosts, Gia knocks and is let in.
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d1", "role": "authenticated"}';
select ok(set_config('t.call1', s.call_id::text, true) is not null and set_config('t.code1', s.code, true) is not null,
          'call 1 starts')
from public.start_call() s;
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d2", "role": "authenticated"}';
select is((select state from public.knock(current_setting('t.code1'))), 'knocking', 'the guest knocks on call 1');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d1", "role": "authenticated"}';
select is(public.answer_knock(current_setting('t.call1')::uuid, '00000000-0000-4000-8000-0000000000d2', true),
          'admitted', 'the host lets the guest into call 1');

-- start_call replaces call 1.
select ok(set_config('t.call2', s.call_id::text, true) is not null and set_config('t.code2', s.code, true) is not null,
          'the host starts call 2')
from public.start_call() s;
select isnt(current_setting('t.code2'), current_setting('t.code1'), 'a new call gets a new code');
select is((public.inbox() -> 'hosting' ->> 'call_id')::uuid, current_setting('t.call2')::uuid,
          'the host''s inbox shows only the new call');
select throws_ok($$select * from public.pass_claims(current_setting('t.call1')::uuid)$$,
  '42501', 'not_allowed', 'the host gets no pass for the replaced call');
reset role;
select isnt((select ended_at from public.calls where id = current_setting('t.call1')::uuid), null,
            'starting a call ends the host''s previous one');
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d2", "role": "authenticated"}';
select throws_ok($$select * from public.pass_claims(current_setting('t.call1')::uuid)$$,
  '42501', 'not_allowed', 'the guest gets no pass for the replaced call');
select throws_ok($$select * from public.knock(current_setting('t.code1'))$$,
  'P0001', 'call_not_found', 'an ended call''s code gives the same error as an unknown one');
select is(public.inbox() -> 'waiting', '[]'::jsonb, 'an ended call leaves the guest''s inbox');

-- Leaving, and knocking again after leaving.
select is((select state from public.knock(current_setting('t.code2'))), 'knocking', 'the guest knocks on call 2');
select lives_ok($$select public.leave_call(current_setting('t.call1')::uuid)$$,
  'leaving a call that already ended is harmless');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d1", "role": "authenticated"}';
select is(public.answer_knock(current_setting('t.call2')::uuid, '00000000-0000-4000-8000-0000000000d2', true),
          'admitted', 'the host lets the guest into call 2');
select throws_ok($$select public.leave_call(current_setting('t.call2')::uuid)$$,
  'P0001', 'not_in_call', 'the host cannot leave their own call (they end it)');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d2", "role": "authenticated"}';
select lives_ok($$select public.leave_call(current_setting('t.call2')::uuid)$$, 'the guest leaves');
select lives_ok($$select public.leave_call(current_setting('t.call2')::uuid)$$, 'leaving twice is a no-op');
select throws_ok($$select * from public.pass_claims(current_setting('t.call2')::uuid)$$,
  '42501', 'not_allowed', 'someone who left gets no pass');
select is((select state from public.knock(current_setting('t.code2'))), 'knocking',
          'someone who left can knock again');
select throws_ok($$select * from public.pass_claims(current_setting('t.call2')::uuid)$$,
  '42501', 'not_allowed', '...and needs letting in again');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d1", "role": "authenticated"}';
select is(public.answer_knock(current_setting('t.call2')::uuid, '00000000-0000-4000-8000-0000000000d2', true),
          'admitted', 'the host lets the guest back in');

-- The heartbeat: only inbox(hosting => call) by the host keeps a call live.
reset role;
update public.calls set seen_at = now() - interval '30 seconds' where id = current_setting('t.call2')::uuid;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d2", "role": "authenticated"}';
select ok(public.inbox(current_setting('t.call2')::uuid) is not null, 'a guest polls, naming the host''s call');
reset role;
select ok((select seen_at < now() from public.calls where id = current_setting('t.call2')::uuid),
          'someone else naming the call is not a heartbeat');
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d1", "role": "authenticated"}';
select is((public.inbox() -> 'hosting' ->> 'call_id')::uuid, current_setting('t.call2')::uuid,
          'a poll without hosting still shows the host''s live call');
reset role;
select ok((select seen_at < now() from public.calls where id = current_setting('t.call2')::uuid),
          'a poll without hosting is not a heartbeat');
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d1", "role": "authenticated"}';
select ok(public.inbox(current_setting('t.call2')::uuid) -> 'hosting' is not null, 'the host polls, naming the call');
reset role;
select is((select seen_at from public.calls where id = current_setting('t.call2')::uuid), now(),
          'naming the call refreshes seen_at');

-- A stale host: not heard from for over 45 s. (Call 1 is ended; its seen_at is set back too, so a
-- refresh would show.)
update public.calls set seen_at = now() - interval '46 seconds' where id = current_setting('t.call2')::uuid;
update public.calls set seen_at = now() - interval '100 seconds' where id = current_setting('t.call1')::uuid;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d2", "role": "authenticated"}';
select throws_ok($$select * from public.pass_claims(current_setting('t.call2')::uuid)$$,
  '42501', 'not_allowed', 'a stale host''s call gives the guest no pass');
select ok(public.inbox(current_setting('t.call2')::uuid) is not null, 'the guest polls naming the stale call');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d3", "role": "authenticated"}';
select throws_ok($$select * from public.knock(current_setting('t.code2'))$$,
  'P0001', 'call_not_found', 'nobody can knock on a stale host''s call');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d1", "role": "authenticated"}';
select throws_ok($$select * from public.pass_claims(current_setting('t.call2')::uuid)$$,
  '42501', 'not_allowed', 'a stale host gets no pass either');
select is(public.inbox() -> 'hosting', 'null'::jsonb, 'a stale call is not shown as hosted');
select throws_ok($$select public.answer_knock(current_setting('t.call2')::uuid, '00000000-0000-4000-8000-0000000000d3', true)$$,
  'P0001', 'call_not_found', 'knocks on a stale call cannot be answered');
select is(public.inbox(gen_random_uuid()) -> 'hosting', 'null'::jsonb, 'an unknown hosting id is ignored');
select is(public.inbox(current_setting('t.call1')::uuid) -> 'hosting', 'null'::jsonb,
          'naming an ended call shows nothing');
reset role;
select ok((select seen_at < now() - interval '45 seconds' from public.calls where id = current_setting('t.call2')::uuid),
          'polls without the id, by a guest, or with a wrong id revive nothing');
select ok((select ended_at is not null and seen_at < now() - interval '45 seconds'
           from public.calls where id = current_setting('t.call1')::uuid),
          'an ended call is never revived');

-- The host's app is still sharing: naming the call brings it back.
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d1", "role": "authenticated"}';
select is((public.inbox(current_setting('t.call2')::uuid) -> 'hosting' ->> 'call_id')::uuid,
          current_setting('t.call2')::uuid, 'naming a stale call revives it');
select is((select role from public.pass_claims(current_setting('t.call2')::uuid)), 'sharer',
          'the host gets passes again');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d2", "role": "authenticated"}';
select is((select identity from public.pass_claims(current_setting('t.call2')::uuid)),
          'guest-00000000-0000-4000-8000-0000000000d2', 'an admitted guest gets passes again');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d3", "role": "authenticated"}';
select is((select state from public.knock(current_setting('t.code2'))), 'knocking',
          'knocking works again');
reset role;

-- Ringing counts as the host acting: it reuses the open call, even a stale one.
update public.calls set seen_at = now() - interval '46 seconds' where id = current_setting('t.call2')::uuid;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d1", "role": "authenticated"}';
select ok(set_config('t.call3', s.call_id::text, true) = current_setting('t.call2'),
          'ringing while the open call is stale reuses it')
from public.ring('00000000-0000-4000-8000-0000000000d2') s;
reset role;
select results_eq(
  $$select ended_at, seen_at from public.calls where id = current_setting('t.call2')::uuid$$,
  $$select null::timestamptz, now()$$,
  '...and brings it back');

-- end_call.
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d2", "role": "authenticated"}';
select is((select role from public.pass_claims(current_setting('t.call3')::uuid)), 'guest', 'the guest is in the call');
select throws_ok($$select public.end_call(current_setting('t.call3')::uuid)$$,
  'P0001', 'call_not_found', 'a guest cannot end the call');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d1", "role": "authenticated"}';
select lives_ok($$select public.end_call(current_setting('t.call3')::uuid)$$, 'the host ends the call');
select lives_ok($$select public.end_call(current_setting('t.call3')::uuid)$$, 'ending it again is a no-op');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d2", "role": "authenticated"}';
select throws_ok($$select * from public.pass_claims(current_setting('t.call3')::uuid)$$,
  '42501', 'not_allowed', 'an ended call gives the guest no pass');
select throws_ok($$select public.leave_call('00000000-0000-4000-8000-000000000000')$$,
  'P0001', 'not_in_call', 'leaving a call you were never in fails');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d1", "role": "authenticated"}';
select throws_ok($$select * from public.pass_claims(current_setting('t.call3')::uuid)$$,
  '42501', 'not_allowed', 'an ended call gives the host no pass');
select ok(set_config('t.code3', (select code from public.calls where id = current_setting('t.call3')::uuid), true) is not null,
          'the host still reads the ended call''s code');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d3", "role": "authenticated"}';
select throws_ok($$select * from public.knock(current_setting('t.code3'))$$,
  'P0001', 'call_not_found', 'nobody can knock on an ended call');
reset role;
update public.calls set seen_at = now() - interval '100 seconds' where id = current_setting('t.call3')::uuid;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000d1", "role": "authenticated"}';
select is(public.inbox(current_setting('t.call3')::uuid) -> 'hosting', 'null'::jsonb,
          'naming an ended call shows nothing');
reset role;
select ok((select seen_at < now() - interval '45 seconds' from public.calls where id = current_setting('t.call3')::uuid),
          'naming an ended call doesn''t refresh it');

select * from finish();
rollback;
