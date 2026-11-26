-- Knocks stay fresh only while the waiting app keeps knocking.
--   e1 Eve (host)   e2 Fin (knocks)   e3 Gus (knocks, turned away)
begin;
create extension if not exists pgtap with schema extensions;
select plan(18);

insert into auth.users (id, email, raw_user_meta_data) values
  ('00000000-0000-4000-8000-0000000000e1', 'eve@example.test', '{"name": "Eve"}'),
  ('00000000-0000-4000-8000-0000000000e2', 'fin@example.test', '{"name": "Fin"}'),
  ('00000000-0000-4000-8000-0000000000e3', 'gus@example.test', '{"name": "Gus"}');

reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000e1", "role": "authenticated"}';
select ok(set_config('t.call', s.call_id::text, true) is not null and set_config('t.code', s.code, true) is not null,
          'the host starts a call')
from public.start_call() s;
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000e2", "role": "authenticated"}';
select is((select state from public.knock(current_setting('t.code'))), 'knocking', 'Fin knocks');

-- Knocking again refreshes the knock, and nothing else.
reset role;
update public.call_members set seen_at = now() - interval '30 seconds', changed_at = now() - interval '30 seconds'
where call_id = current_setting('t.call')::uuid and user_id = '00000000-0000-4000-8000-0000000000e2';
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000e2", "role": "authenticated"}';
select is((select state from public.knock(current_setting('t.code'))), 'knocking', 'Fin knocks again');
reset role;
select results_eq(
  $$select seen_at, changed_at from public.call_members where call_id = current_setting('t.call')::uuid and user_id = '00000000-0000-4000-8000-0000000000e2'$$,
  $$select now(), now() - interval '30 seconds'$$,
  'a repeated knock refreshes seen_at and leaves changed_at');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000e1", "role": "authenticated"}';
select results_eq(
  $$select m ->> 'name', m ->> 'state' from jsonb_array_elements(public.inbox(current_setting('t.call')::uuid) -> 'hosting' -> 'members') m$$,
  $$values ('Fin', 'knocking')$$,
  'the host sees a fresh knock');

-- A knock not repeated for 45 s drops out.
reset role;
update public.call_members set seen_at = now() - interval '46 seconds'
where call_id = current_setting('t.call')::uuid and user_id = '00000000-0000-4000-8000-0000000000e2';
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000e1", "role": "authenticated"}';
select is((public.inbox(current_setting('t.call')::uuid) -> 'hosting' -> 'members'), '[]'::jsonb,
          'a knock not seen for 45 s is left out of the host''s list');
select throws_ok(
  $$select public.answer_knock(current_setting('t.call')::uuid, '00000000-0000-4000-8000-0000000000e2', true)$$,
  'P0001', 'not_knocking: nobody is waiting with that id', 'the host cannot let in someone who has gone');

-- Knocking again makes it fresh.
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000e2", "role": "authenticated"}';
select is((select state from public.knock(current_setting('t.code'))), 'knocking', 'Fin comes back and knocks');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000e1", "role": "authenticated"}';
select results_eq(
  $$select m ->> 'name', m ->> 'state' from jsonb_array_elements(public.inbox(current_setting('t.call')::uuid) -> 'hosting' -> 'members') m$$,
  $$values ('Fin', 'knocking')$$,
  'the renewed knock shows up again');
select is(public.answer_knock(current_setting('t.call')::uuid, '00000000-0000-4000-8000-0000000000e2', true),
          'admitted', 'and can be answered');

-- Admitted and turned-away rows don't depend on seen_at, and repeat knocks don't touch it.
reset role;
update public.call_members set seen_at = now() - interval '100 seconds'
where call_id = current_setting('t.call')::uuid and user_id = '00000000-0000-4000-8000-0000000000e2';
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000e2", "role": "authenticated"}';
select is((select state from public.knock(current_setting('t.code'))), 'admitted',
          'a repeat knock by an admitted person returns admitted');
reset role;
select ok((select seen_at < now() - interval '45 seconds' from public.call_members where call_id = current_setting('t.call')::uuid and user_id = '00000000-0000-4000-8000-0000000000e2'),
          '...without refreshing seen_at');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000e3", "role": "authenticated"}';
select is((select state from public.knock(current_setting('t.code'))), 'knocking', 'Gus knocks');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000e1", "role": "authenticated"}';
select is(public.answer_knock(current_setting('t.call')::uuid, '00000000-0000-4000-8000-0000000000e3', false),
          'turned_away', 'the host turns Gus away');
reset role;
update public.call_members set seen_at = now() - interval '100 seconds'
where call_id = current_setting('t.call')::uuid and user_id = '00000000-0000-4000-8000-0000000000e3';
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000e3", "role": "authenticated"}';
select is((select state from public.knock(current_setting('t.code'))), 'turned_away',
          'a repeat knock by a turned-away person returns turned_away');
reset role;
select ok((select seen_at < now() - interval '45 seconds' from public.call_members where call_id = current_setting('t.call')::uuid and user_id = '00000000-0000-4000-8000-0000000000e3'),
          '...without refreshing seen_at');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000e1", "role": "authenticated"}';
select results_eq(
  $$select m ->> 'name', m ->> 'state' from jsonb_array_elements(public.inbox(current_setting('t.call')::uuid) -> 'hosting' -> 'members') m$$,
  $$values ('Fin', 'admitted'), ('Gus', 'turned_away')$$,
  'admitted and turned-away people stay in the host''s list however long ago they knocked');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000e2", "role": "authenticated"}';
select is((select role from public.pass_claims(current_setting('t.call')::uuid)), 'guest',
          'the admitted person''s pass doesn''t depend on knocking');

select * from finish();
rollback;
