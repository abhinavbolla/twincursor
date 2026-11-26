-- A stalled call can come back for 10 minutes, and never after.
--   f1 Ivy (host)   f2 Jon (admitted guest)   f3 Lea (a contact Ivy rings)
begin;
create extension if not exists pgtap with schema extensions;
select plan(21);

insert into auth.users (id, email, raw_user_meta_data) values
  ('00000000-0000-4000-8000-0000000000f1', 'ivy@example.test', '{"name": "Ivy"}'),
  ('00000000-0000-4000-8000-0000000000f2', 'jon@example.test', '{"name": "Jon"}'),
  ('00000000-0000-4000-8000-0000000000f3', 'lea@example.test', '{"name": "Lea"}');
insert into public.contacts (owner, contact) values
  ('00000000-0000-4000-8000-0000000000f1', '00000000-0000-4000-8000-0000000000f3'),
  ('00000000-0000-4000-8000-0000000000f3', '00000000-0000-4000-8000-0000000000f1');

reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f1", "role": "authenticated"}';
select ok(set_config('t.a', s.call_id::text, true) is not null and set_config('t.code_a', s.code, true) is not null, 'the host starts call a') from public.start_call() s;
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f2", "role": "authenticated"}';
select is((select state from public.knock(current_setting('t.code_a'))), 'knocking',
          'the guest knocks on call a');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f1", "role": "authenticated"}';
select is(public.answer_knock(current_setting('t.a')::uuid, '00000000-0000-4000-8000-0000000000f2', true), 'admitted',
          'the host lets the guest into call a');

-- Silent for 9 minutes: the heartbeat brings it back.
reset role;
update public.calls set seen_at = now() - interval '9 minutes' where id = current_setting('t.a')::uuid;
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f2", "role": "authenticated"}';
select throws_ok($$select * from public.pass_claims(current_setting('t.a')::uuid)$$, '42501', 'not_allowed',
  'a call silent for 9 minutes gives no pass while stale');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f1", "role": "authenticated"}';
select is((public.inbox(current_setting('t.a')::uuid) -> 'hosting' ->> 'call_id')::uuid, current_setting('t.a')::uuid,
          'a call silent for 9 minutes revives through inbox(hosting)');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f2", "role": "authenticated"}';
select is((select identity from public.pass_claims(current_setting('t.a')::uuid)), 'guest-00000000-0000-4000-8000-0000000000f2',
          '...and its admitted guest gets a pass again');

-- Silent for 9 minutes again: a ring brings it back.
reset role;
update public.calls set seen_at = now() - interval '9 minutes' where id = current_setting('t.a')::uuid;
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f1", "role": "authenticated"}';
select is((select call_id from public.ring('00000000-0000-4000-8000-0000000000f3')), current_setting('t.a')::uuid,
          'a call silent for 9 minutes revives through ring');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f2", "role": "authenticated"}';
select is((select role from public.pass_claims(current_setting('t.a')::uuid)), 'guest',
          '...and its admitted guest gets a pass again');

-- Silent for 11 minutes: over, through the heartbeat.
reset role;
update public.calls set seen_at = now() - interval '11 minutes' where id = current_setting('t.a')::uuid;
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f1", "role": "authenticated"}';
select is(public.inbox(current_setting('t.a')::uuid) -> 'hosting', 'null'::jsonb,
          'a call silent for 11 minutes does not revive through inbox(hosting)');
reset role;
select ok((select ended_at is not null and seen_at < now() - interval '10 minutes'
           from public.calls where id = current_setting('t.a')::uuid),
          '...is marked ended, and its heartbeat is not refreshed');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f2", "role": "authenticated"}';
select throws_ok($$select * from public.pass_claims(current_setting('t.a')::uuid)$$, '42501', 'not_allowed',
  '...and its admitted guest gets no pass');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f1", "role": "authenticated"}';
select is(public.inbox(current_setting('t.a')::uuid) -> 'hosting', 'null'::jsonb, 'naming it again still revives nothing');
select ok(set_config('t.b', s.call_id::text, true) <> current_setting('t.a') and set_config('t.code_b', s.code, true) is not null,
          'with the old call over, the host can start another')
from public.start_call() s;
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f2", "role": "authenticated"}';
select is((select state from public.knock(current_setting('t.code_b'))), 'knocking',
          'the guest knocks on call b');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f1", "role": "authenticated"}';
select is(public.answer_knock(current_setting('t.b')::uuid, '00000000-0000-4000-8000-0000000000f2', true), 'admitted',
          'the host lets the guest into call b');

-- Silent for 11 minutes: over, through a ring.
reset role;
update public.calls set seen_at = now() - interval '11 minutes' where id = current_setting('t.b')::uuid;
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f1", "role": "authenticated"}';
select ok(set_config('t.c', s.call_id::text, true) <> current_setting('t.b'),
          'ringing after 11 minutes of silence starts a new call')
from public.ring('00000000-0000-4000-8000-0000000000f3') s;
reset role;
select ok((select ended_at is not null from public.calls where id = current_setting('t.b')::uuid),
          '...ends the old one');
select results_eq(
  $$select user_id, state from public.call_members where call_id = current_setting('t.c')::uuid$$,
  $$values ('00000000-0000-4000-8000-0000000000f3'::uuid, 'ringing'::text)$$,
  '...and the new call has none of the old members');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f2", "role": "authenticated"}';
select throws_ok($$select * from public.pass_claims(current_setting('t.b')::uuid)$$, '42501', 'not_allowed',
  'the old call''s admitted guest gets no pass for it');
select throws_ok($$select * from public.pass_claims(current_setting('t.c')::uuid)$$, '42501', 'not_allowed',
  '...nor for the new call');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000f1", "role": "authenticated"}';
select is((public.inbox(current_setting('t.b')::uuid) -> 'hosting' ->> 'call_id')::uuid, current_setting('t.c')::uuid,
          'naming the ended call refreshes nothing; the inbox shows the new call');

select * from finish();
rollback;
