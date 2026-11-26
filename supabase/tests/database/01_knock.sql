-- Knocking with a call link, answering, and who gets a call pass.
--   b1 Hana (host)   b2 Kai (knocks, let in)   b3 Sam (stranger)   b4 Olu (knocks, turned away)
begin;
create extension if not exists pgtap with schema extensions;
select plan(46);

insert into auth.users (id, email, raw_user_meta_data) values
  ('00000000-0000-4000-8000-0000000000b1', 'hana@example.test', '{"name": "Hana Host"}'),
  ('00000000-0000-4000-8000-0000000000b2', 'kai@example.test', '{"name": "Kai Knocker"}'),
  ('00000000-0000-4000-8000-0000000000b3', 'sam@example.test', '{"name": "Sam Stranger"}'),
  ('00000000-0000-4000-8000-0000000000b4', 'olu@example.test', '{"name": "Olu Other"}');

-- Hana starts a call.
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000b1", "role": "authenticated"}';
select ok(set_config('t.call', s.call_id::text, true) is not null and set_config('t.code', s.code, true) is not null,
          'the host starts a call')
from public.start_call() s;
select ok(current_setting('t.code') ~ '^[0-9a-f]{32}$', 'a call code is 32 lowercase hex characters');
select ok(
  (select room <> code from public.calls where id = current_setting('t.call')::uuid),
  'the room name is not the code');
select throws_ok(
  $$select * from public.knock(current_setting('t.code'))$$,
  '22023', 'own_call: you host this call', 'the host cannot knock on their own call');

-- Kai knocks.
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000b2", "role": "authenticated"}';
select results_eq(
  $$select state, host_id, host_name from public.knock(current_setting('t.code'))$$,
  $$values ('knocking'::text, '00000000-0000-4000-8000-0000000000b1'::uuid, 'Hana Host'::text)$$,
  'knocking returns knocking and the host''s profile');
select is(
  (select state from public.knock(current_setting('t.code'))),
  'knocking', 'knocking again changes nothing');
select is(
  (select name from public.profiles where id = '00000000-0000-4000-8000-0000000000b1'),
  'Hana Host', 'the knocker can read the host''s profile');
select results_eq(
  $$select (w ->> 'state'), (w -> 'host' ->> 'name')
    from jsonb_array_elements(public.inbox() -> 'waiting') w$$,
  $$values ('knocking', 'Hana Host')$$,
  'the knocker''s inbox shows the knock as waiting');
select throws_ok(
  $$select * from public.pass_claims(current_setting('t.call')::uuid)$$,
  '42501', 'not_allowed', 'a knocker gets no call pass');
select throws_ok(
  $$select public.answer_knock(current_setting('t.call')::uuid, '00000000-0000-4000-8000-0000000000b2', true)$$,
  'P0001', 'call_not_found', 'a knocker cannot let themselves in');
select throws_ok(
  $$select * from public.knock('00000000000000000000000000000000')$$,
  'P0001', 'call_not_found', 'an unknown code gives call_not_found');
select throws_ok(
  $$select * from public.knock(null)$$,
  'P0001', 'call_not_found', 'a null code gives call_not_found');

reset role;
select is(
  (select count(*)::int from public.call_members where call_id = current_setting('t.call')::uuid),
  1, 'two knocks made one member row');

-- Hana sees Kai waiting.
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000b1", "role": "authenticated"}';
select results_eq(
  $$select (m ->> 'user_id')::uuid, m ->> 'name', m ->> 'state'
    from jsonb_array_elements(public.inbox() -> 'hosting' -> 'members') m$$,
  $$values ('00000000-0000-4000-8000-0000000000b2'::uuid, 'Kai Knocker', 'knocking')$$,
  'the host''s inbox lists the knocker with their profile');
select is(
  (select name from public.profiles where id = '00000000-0000-4000-8000-0000000000b2'),
  'Kai Knocker', 'the host can read the knocker''s profile');
select results_eq(
  $$select identity, role, name from public.pass_claims(current_setting('t.call')::uuid)$$,
  $$values ('sharer-00000000-0000-4000-8000-0000000000b1', 'sharer', 'Hana Host')$$,
  'the host gets a sharer pass');
select throws_ok(
  $$select public.answer_knock(current_setting('t.call')::uuid, '00000000-0000-4000-8000-0000000000b2', null)$$,
  '22023', 'bad_argument: let_in is required', 'answering needs a yes or no');
select throws_ok(
  $$select public.answer_knock(current_setting('t.call')::uuid, '00000000-0000-4000-8000-0000000000b3', true)$$,
  'P0001', 'not_knocking: nobody is waiting with that id', 'the host cannot admit someone who never knocked');
select is(
  public.answer_knock(current_setting('t.call')::uuid, '00000000-0000-4000-8000-0000000000b2', true),
  'admitted', 'the host lets the knocker in');
select throws_ok(
  $$select public.answer_knock(current_setting('t.call')::uuid, '00000000-0000-4000-8000-0000000000b2', false)$$,
  'P0001', 'not_knocking: nobody is waiting with that id', 'a knock can be answered only once');
select is(
  (public.inbox() -> 'contacts' -> 0 ->> 'name'),
  'Kai Knocker', 'the admitted person is now in the host''s contacts');

-- Kai is in.
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000b2", "role": "authenticated"}';
select results_eq(
  $$select identity, role, name, room from public.pass_claims(current_setting('t.call')::uuid)$$,
  $$select 'guest-00000000-0000-4000-8000-0000000000b2'::text, 'guest'::text, 'Kai Knocker'::text, c.room
    from public.calls c where c.id = current_setting('t.call')::uuid$$,
  'once admitted, the knocker gets a guest pass for the call''s room');
select is(
  (select state from public.knock(current_setting('t.code'))),
  'admitted', 'knocking again once admitted stays admitted');
select results_eq(
  $$select (w ->> 'state') from jsonb_array_elements(public.inbox() -> 'waiting') w$$,
  $$values ('admitted')$$,
  'the inbox tells the knocker they were let in');
select is(
  (public.inbox() -> 'contacts' -> 0 ->> 'name'),
  'Hana Host', 'the host is now in the guest''s contacts');

-- Kai hosting a call of his own changes nothing about his place in Hana's.
select ok(set_config('t.kai_call', s.call_id::text, true) is not null, 'the guest starts a call of their own')
from public.start_call() s;
select is(
  (select role from public.pass_claims(current_setting('t.call')::uuid)),
  'guest', 'a guest who hosts elsewhere still gets only a guest pass here');

-- Hana can't touch Kai's call.
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000b1", "role": "authenticated"}';
select throws_ok(
  $$select * from public.pass_claims(current_setting('t.kai_call')::uuid)$$,
  '42501', 'not_allowed', 'a host gets no pass for someone else''s call');
select throws_ok(
  $$select public.answer_knock(current_setting('t.kai_call')::uuid, '00000000-0000-4000-8000-0000000000b2', true)$$,
  'P0001', 'call_not_found', 'a host cannot answer knocks on someone else''s call');
select throws_ok(
  $$select public.end_call(current_setting('t.kai_call')::uuid)$$,
  'P0001', 'call_not_found', 'a host cannot end someone else''s call');

-- Olu knocks and is turned away.
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000b4", "role": "authenticated"}';
select is((select state from public.knock(current_setting('t.code'))), 'knocking', 'a second person knocks');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000b1", "role": "authenticated"}';
select is(
  public.answer_knock(current_setting('t.call')::uuid, '00000000-0000-4000-8000-0000000000b4', false),
  'turned_away', 'the host turns them away');
select throws_ok(
  $$select public.answer_knock(current_setting('t.call')::uuid, '00000000-0000-4000-8000-0000000000b4', true)$$,
  'P0001', 'not_knocking: nobody is waiting with that id', 'a turned-away knock cannot be admitted later');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000b4", "role": "authenticated"}';
select is((select state from public.knock(current_setting('t.code'))), 'turned_away',
          'a turned-away person cannot knock again');
select throws_ok(
  $$select * from public.pass_claims(current_setting('t.call')::uuid)$$,
  '42501', 'not_allowed', 'a turned-away person gets no pass');
select results_eq(
  $$select (w ->> 'state') from jsonb_array_elements(public.inbox() -> 'waiting') w$$,
  $$values ('turned_away')$$,
  'the inbox tells them they were turned away');
select is(
  (select count(*)::int from public.profiles where id = '00000000-0000-4000-8000-0000000000b1'),
  0, 'once turned away, they can no longer read the host''s profile');
select is((public.inbox() -> 'contacts'), '[]'::jsonb, 'turning someone away doesn''t make contacts');

-- Sam, a stranger, sees nothing and gets nothing.
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000b3", "role": "authenticated"}';
select results_eq(
  $$select id from public.profiles$$,
  $$values ('00000000-0000-4000-8000-0000000000b3'::uuid)$$,
  'a stranger reads only their own profile');
select is((select count(*)::int from public.calls), 0, 'a stranger reads no call');
select is((select count(*)::int from public.call_members), 0, 'a stranger reads no member row');
select is((select count(*)::int from public.contacts), 0, 'a stranger reads no contact');
select throws_ok(
  $$select * from public.pass_claims(current_setting('t.call')::uuid)$$,
  '42501', 'not_allowed', 'a stranger gets no pass');
select throws_ok(
  $$select * from public.pass_claims(null)$$,
  '42501', 'not_allowed', 'a null call id gets no pass');
select throws_ok(
  $$select public.answer_knock(current_setting('t.call')::uuid, '00000000-0000-4000-8000-0000000000b4', true)$$,
  'P0001', 'call_not_found', 'a stranger cannot answer knocks');
select results_eq(
  $$select public.inbox() - 'me'$$,
  $$values ('{"contacts": [], "hosting": null, "rings": [], "waiting": []}'::jsonb)$$,
  'a stranger''s inbox is empty');

select * from finish();
rollback;
