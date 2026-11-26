-- Ringing a contact, answering a ring, and removing a contact.
--   c1 Ana (host)   c2 Ben (contact both ways)   c3 Cy (in Ana's contacts, but Ana isn't in his)
--   c4 Dee (nobody's contact)
begin;
create extension if not exists pgtap with schema extensions;
select plan(34);

insert into auth.users (id, email, raw_user_meta_data) values
  ('00000000-0000-4000-8000-0000000000c1', 'ana@example.test', '{"name": "Ana"}'),
  ('00000000-0000-4000-8000-0000000000c2', 'ben@example.test', '{"name": "Ben"}'),
  ('00000000-0000-4000-8000-0000000000c3', 'cy@example.test', '{"name": "Cy"}'),
  ('00000000-0000-4000-8000-0000000000c4', 'dee@example.test', '{"name": "Dee"}');
insert into public.contacts (owner, contact) values
  ('00000000-0000-4000-8000-0000000000c1', '00000000-0000-4000-8000-0000000000c2'),
  ('00000000-0000-4000-8000-0000000000c2', '00000000-0000-4000-8000-0000000000c1'),
  ('00000000-0000-4000-8000-0000000000c1', '00000000-0000-4000-8000-0000000000c3');

set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000c1", "role": "authenticated"}';
select throws_ok($$select * from public.ring('00000000-0000-4000-8000-0000000000c4')$$,
  '42501', 'not_a_contact', 'ringing someone who isn''t a contact fails');
select throws_ok($$select * from public.ring('00000000-0000-4000-8000-0000000000c3')$$,
  '42501', 'not_a_contact', 'ringing someone who doesn''t have you as a contact fails');
select throws_ok($$select * from public.ring('00000000-0000-4000-8000-0000000000c1')$$,
  '42501', 'not_a_contact', 'ringing yourself fails');
select throws_ok($$select * from public.ring(null)$$,
  '42501', 'not_a_contact', 'ringing nobody fails');
select is(public.inbox() -> 'hosting', 'null'::jsonb, 'a failed ring starts no call');

select ok(set_config('t.call', s.call_id::text, true) is not null and set_config('t.code', s.code, true) is not null,
          'ringing a contact starts a call')
from public.ring('00000000-0000-4000-8000-0000000000c2') s;
select results_eq(
  $$select (public.inbox() -> 'hosting' ->> 'call_id')::uuid, public.inbox() -> 'hosting' ->> 'code'$$,
  $$select current_setting('t.call')::uuid, current_setting('t.code')$$,
  'the host''s inbox shows the call the ring started');
select results_eq(
  $$select m ->> 'name', m ->> 'state' from jsonb_array_elements(public.inbox() -> 'hosting' -> 'members') m$$,
  $$values ('Ben', 'ringing')$$,
  'the host sees the contact ringing');
select throws_ok($$select public.answer_ring(current_setting('t.call')::uuid, true)$$,
  'P0001', 'no_pending_ring', 'the host cannot answer their own ring');

-- Strangers can't answer it.
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000c4", "role": "authenticated"}';
select throws_ok($$select public.answer_ring(current_setting('t.call')::uuid, true)$$,
  'P0001', 'no_pending_ring', 'someone who isn''t being rung cannot answer');
select is(public.inbox() -> 'rings', '[]'::jsonb, 'someone else sees no ring');

-- Ben sees the ring, and declines.
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000c2", "role": "authenticated"}';
select results_eq(
  $$select (r ->> 'call_id')::uuid, r -> 'host' ->> 'name' from jsonb_array_elements(public.inbox() -> 'rings') r$$,
  $$select current_setting('t.call')::uuid, 'Ana'::text$$,
  'the contact''s inbox shows who is ringing');
select throws_ok($$select * from public.pass_claims(current_setting('t.call')::uuid)$$,
  '42501', 'not_allowed', 'a ringing person gets no pass before answering');
select throws_ok($$select public.answer_ring(current_setting('t.call')::uuid, null)$$,
  '22023', 'bad_argument: accept is required', 'answering a ring needs a yes or no');
select is(public.answer_ring(current_setting('t.call')::uuid, false), 'declined', 'the contact declines');
select throws_ok($$select * from public.pass_claims(current_setting('t.call')::uuid)$$,
  '42501', 'not_allowed', 'a declined ring gives no pass');
select throws_ok($$select public.answer_ring(current_setting('t.call')::uuid, true)$$,
  'P0001', 'no_pending_ring', 'a declined ring cannot be accepted afterwards');
select is(public.inbox() -> 'rings', '[]'::jsonb, 'a declined ring leaves the inbox');
select is((select state from public.knock(current_setting('t.code'))), 'declined',
          'someone who declined cannot knock their way in instead');

-- Ana rings again; this time it goes unanswered for over a minute.
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000c1", "role": "authenticated"}';
select is((select call_id from public.ring('00000000-0000-4000-8000-0000000000c2')),
          current_setting('t.call')::uuid, 'ringing again uses the live call');
reset role;
update public.call_members set changed_at = now() - interval '61 seconds'
where call_id = current_setting('t.call')::uuid;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000c1", "role": "authenticated"}';
select is(public.inbox() -> 'hosting' -> 'members' -> 0 ->> 'state', 'missed',
          'the host sees an unanswered ring as missed after 60 s');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000c2", "role": "authenticated"}';
select is(public.inbox() -> 'rings', '[]'::jsonb, 'a ring older than 60 s leaves the inbox');
select throws_ok($$select public.answer_ring(current_setting('t.call')::uuid, true)$$,
  'P0001', 'no_pending_ring', 'a ring older than 60 s cannot be accepted');

-- A third ring, accepted.
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000c1", "role": "authenticated"}';
select ok(s.call_id is not null, 'the host rings once more') from public.ring('00000000-0000-4000-8000-0000000000c2') s;
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000c2", "role": "authenticated"}';
select is(public.answer_ring(current_setting('t.call')::uuid, true), 'admitted', 'the contact accepts');
select results_eq(
  $$select identity, role from public.pass_claims(current_setting('t.call')::uuid)$$,
  $$values ('guest-00000000-0000-4000-8000-0000000000c2', 'guest')$$,
  'an accepted ring gives a guest pass');
select throws_ok($$select public.answer_ring(current_setting('t.call')::uuid, false)$$,
  'P0001', 'no_pending_ring', 'a ring can be answered only once');

reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000c1", "role": "authenticated"}';
select ok(s.call_id is not null, 'ringing someone already in the call works')
from public.ring('00000000-0000-4000-8000-0000000000c2') s;
reset role;
select is(
  (select state from public.call_members
   where call_id = current_setting('t.call')::uuid and user_id = '00000000-0000-4000-8000-0000000000c2'),
  'admitted', '...and leaves them admitted');

-- Ben removes Ana. Ana can't ring him any more; Ben still can't be rung by a non-contact.
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000c2", "role": "authenticated"}';
select lives_ok($$select public.remove_contact('00000000-0000-4000-8000-0000000000c1')$$, 'a contact can be removed');
select is(public.inbox() -> 'contacts', '[]'::jsonb, 'the removed contact is gone from the inbox');
select lives_ok($$select public.remove_contact('00000000-0000-4000-8000-0000000000c4')$$,
  'removing someone who isn''t a contact is a no-op');
reset role;
select is(
  (select count(*)::int from public.contacts where owner = '00000000-0000-4000-8000-0000000000c1'),
  2, 'removing a contact removes only your own side');
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000c1", "role": "authenticated"}';
select throws_ok($$select * from public.ring('00000000-0000-4000-8000-0000000000c2')$$,
  '42501', 'not_a_contact', 'someone who removed you can no longer be rung by you');

select * from finish();
rollback;
