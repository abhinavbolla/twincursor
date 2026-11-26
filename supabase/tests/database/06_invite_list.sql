-- Who may start calls: the invite list, the invite_only setting, asking to host, the waiting list.
--   1a1 Alex (GitHub "Alex-Example", invited)    1a2 Sam (GitHub "sam-example", never invited)
--   1a3 Kim (no GitHub identity, so no handle)   1a4 Pat (GitHub "pat-example", invited before signing up)
--   1a5 Lee (GitHub "lee-example", invited, then removed while hosting a call)
begin;
create extension if not exists pgtap with schema extensions;
select plan(82);

-------------------------------------------------------------------------------------------------
-- What the migration leaves behind, and who can reach it
-------------------------------------------------------------------------------------------------

select results_eq(
  $$select id, invite_only from public.server_settings$$,
  $$values (true, false)$$,
  'the migration leaves one settings row, with the server open');
select is(
  (select array_agg(version::text order by version) from supabase_migrations.schema_migrations),
  array['20261126120000', '20261127120000'],
  'the invite list is a migration of its own, applied after the first one');
select ok(
  (select bool_and(c.relrowsecurity) from pg_class c
   where c.oid in ('public.invited'::regclass, 'public.server_settings'::regclass, 'public.host_requests'::regclass)),
  'row-level security is on for the three new tables');
select is(
  (select count(*)::int from pg_policies
   where schemaname = 'public' and tablename in ('invited', 'server_settings', 'host_requests')),
  0, 'the new tables have no policy at all');
select is(
  (select count(*)::int
   from unnest(array['public.invited', 'public.server_settings', 'public.host_requests', 'public.waiting_to_host']) t(name),
        unnest(array['SELECT', 'INSERT', 'UPDATE', 'DELETE', 'TRUNCATE', 'REFERENCES', 'TRIGGER']) p(priv),
        unnest(array['anon', 'authenticated']) r(role)
   where has_table_privilege(r.role, t.name, p.priv)),
  0, 'anon and authenticated hold no privilege on the new tables or the view');
select ok(
  (select 'security_invoker=true' = any(c.reloptions) from pg_class c where c.oid = 'public.waiting_to_host'::regclass),
  'the waiting list reads with the reader''s rights (security_invoker)');
select ok(
  not has_function_privilege('authenticated', 'private.can_host(uuid)', 'EXECUTE')
    and not has_function_privilege('authenticated', 'private.require_host(uuid)', 'EXECUTE'),
  'clients cannot call the private host check');

-- Pat is invited before Pat has an account, in capitals and with an @, as an owner might type it.
insert into public.invited (handle, note) values (' @PAT-Example ', 'invited before signing up');
select is((select handle from public.invited), 'pat-example',
  'a handle is stored lowercased, without the @ or spaces');
select throws_ok($$insert into public.invited (handle) values ('not a handle!')$$,
  '23514', null, 'a handle with characters GitHub does not allow is refused');
select throws_ok($$insert into public.invited (handle) values ('pat-EXAMPLE')$$,
  '23505', null, 'the same handle in other letters is the same handle');
delete from public.invited;

-- People made the way existing accounts were: users first, then GitHub identities.
insert into auth.users (id, email, raw_user_meta_data) values
  ('00000000-0000-4000-8000-0000000001a1', 'alex@example.test', '{}'),
  ('00000000-0000-4000-8000-0000000001a2', 'sam@example.test', '{}'),
  ('00000000-0000-4000-8000-0000000001a3', 'kim@example.test', '{"name": "Kim", "user_name": "kim-example"}'),
  ('00000000-0000-4000-8000-0000000001a5', 'lee@example.test', '{}');
insert into auth.identities (provider_id, user_id, identity_data, provider) values
  ('2001', '00000000-0000-4000-8000-0000000001a1', '{"sub": "2001", "user_name": "Alex-Example", "name": "Alex"}', 'github'),
  ('2002', '00000000-0000-4000-8000-0000000001a2', '{"sub": "2002", "user_name": "sam-example", "name": "Sam"}', 'github'),
  ('2005', '00000000-0000-4000-8000-0000000001a5', '{"sub": "2005", "user_name": "lee-example", "name": "Lee"}', 'github');
insert into public.contacts (owner, contact) values
  ('00000000-0000-4000-8000-0000000001a1', '00000000-0000-4000-8000-0000000001a2'),
  ('00000000-0000-4000-8000-0000000001a2', '00000000-0000-4000-8000-0000000001a1');

-- Clients can't reach any of it, signed in or not.
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a2", "role": "authenticated"}';
select throws_ok($$select * from public.invited$$, '42501', null, 'a signed-in client cannot read the invite list');
select throws_ok($$select * from public.server_settings$$, '42501', null, 'a signed-in client cannot read the setting');
select throws_ok($$select * from public.host_requests$$, '42501', null, 'a signed-in client cannot read who asked');
select throws_ok($$select * from public.waiting_to_host$$, '42501', null, 'a signed-in client cannot read the waiting list');
select throws_ok($$insert into public.invited (handle) values ('sam-example')$$, '42501', null,
  'a signed-in client cannot invite themselves');
select throws_ok($$update public.server_settings set invite_only = false$$, '42501', null,
  'a signed-in client cannot change the setting');
select throws_ok($$insert into public.host_requests (user_id) values ('00000000-0000-4000-8000-0000000001a2')$$, '42501', null,
  'a signed-in client cannot write a request directly');
reset role;
set local role anon;
set local request.jwt.claims to '{"role": "anon"}';
select throws_ok($$select * from public.invited$$, '42501', null, 'anon cannot read the invite list');
select throws_ok($$select * from public.waiting_to_host$$, '42501', null, 'anon cannot read the waiting list');
select throws_ok($$select public.ask_to_host()$$, '42501', null, 'anon cannot ask to host');
reset role;

-------------------------------------------------------------------------------------------------
-- An open server: everyone signed in may start calls
-------------------------------------------------------------------------------------------------

set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a2", "role": "authenticated"}';
select results_eq(
  $$select (public.inbox() -> 'me' ->> 'can_host')::boolean, (public.inbox() -> 'me' ->> 'asked_to_host')::boolean$$,
  $$values (true, false)$$,
  'open: someone not on the list can host, and has not asked');
select is(
  (select array_agg(k order by k) from jsonb_object_keys(public.inbox() -> 'me') k),
  array['asked_to_host', 'avatar_url', 'can_host', 'handle', 'id', 'name'],
  'inbox().me keeps the profile and adds can_host and asked_to_host');
select ok(s.call_id is not null, 'open: someone not on the list starts a call') from public.start_call() s;
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a3", "role": "authenticated"}';
select ok(s.call_id is not null, 'open: someone without a GitHub handle starts a call') from public.start_call() s;
reset role;
update public.calls set ended_at = now() where ended_at is null;

-------------------------------------------------------------------------------------------------
-- The owner's safety checks
-------------------------------------------------------------------------------------------------

select throws_ok($$update public.server_settings set invite_only = true$$,
  'P0001', 'invite_list_empty: add your own GitHub handle to public.invited before turning invite_only on, or nobody could start a call',
  'invite_only cannot be turned on while the list is empty');
select throws_ok($$delete from public.server_settings$$,
  'P0001', 'keep_server_settings: this row holds the server''s settings; set invite_only to false instead of deleting it',
  'the settings row cannot be deleted');
select throws_ok($$truncate public.server_settings$$, 'P0001', null, 'the settings table cannot be emptied');
select throws_ok($$insert into public.server_settings (id, invite_only) values (false, false)$$,
  '23514', null, 'there is never a second settings row');

insert into public.invited (handle, note) values
  ('alex-example', 'me, the owner'), ('PAT-Example', null), ('lee-example', null);
select lives_ok($$update public.server_settings set invite_only = true$$,
  'with handles on the list, invite_only turns on');

select throws_ok($$delete from public.invited$$,
  'P0001', 'last_invited: invite_only is on and this would empty the invite list, so nobody could start a call; turn invite_only off first',
  'the whole list cannot be deleted at once while invite_only is on');
select throws_ok($$truncate public.invited$$, 'P0001', null, 'the list cannot be truncated while invite_only is on');
select is((select count(*)::int from public.invited), 3, '...and nothing was removed');
select lives_ok($$delete from public.invited where handle = 'pat-example'$$, 'one of several handles can be removed');
select lives_ok($$delete from public.invited where handle in ('lee-example')$$, 'and another');
select throws_ok($$delete from public.invited where handle = 'alex-example'$$, 'P0001', null,
  'the last handle cannot be removed while invite_only is on');
insert into public.invited (handle) values ('pat-example'), ('lee-example');

-- The service role (what an owner's script would use) may manage all of it.
set local role service_role;
select lives_ok($$select * from public.waiting_to_host$$, 'the service role can read the waiting list');
select lives_ok($$update public.invited set note = 'still me' where handle = 'alex-example'$$,
  'the service role can edit the invite list');
reset role;

-------------------------------------------------------------------------------------------------
-- Invite-only: who may start calls
-------------------------------------------------------------------------------------------------

-- Alex, whose GitHub login has capitals.
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a1", "role": "authenticated"}';
select is((public.inbox() -> 'me' ->> 'can_host')::boolean, true, 'invite-only: an invited handle can host, whatever its letter case');
select ok(set_config('t.call', s.call_id::text, true) is not null and set_config('t.code', s.code, true) is not null,
          'invite-only: an invited person starts a call')
from public.start_call() s;

-- Sam, not invited.
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a2", "role": "authenticated"}';
select results_eq(
  $$select (public.inbox() -> 'me' ->> 'can_host')::boolean, (public.inbox() -> 'me' ->> 'asked_to_host')::boolean$$,
  $$values (false, false)$$,
  'invite-only: someone not on the list cannot host, and has not asked');
select throws_ok($$select * from public.start_call()$$, '42501', 'not_invited: this server is invite-only',
  'invite-only: someone not on the list cannot start a call');
select throws_ok($$select * from public.ring('00000000-0000-4000-8000-0000000001a1')$$, '42501',
  'not_invited: this server is invite-only', 'invite-only: someone not on the list cannot ring, even a contact');
select is(public.inbox() -> 'hosting', 'null'::jsonb, 'the refused start and ring left no call behind');

-- ...but joins Alex's call like anyone: knock, let in, guest pass.
select is((select state from public.knock(current_setting('t.code'))), 'knocking',
  'someone not on the list can knock on an invited host''s call');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a1", "role": "authenticated"}';
select is(public.answer_knock(current_setting('t.call')::uuid, '00000000-0000-4000-8000-0000000001a2', true), 'admitted',
  'the invited host lets them in');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a2", "role": "authenticated"}';
select results_eq(
  $$select identity, role from public.pass_claims(current_setting('t.call')::uuid)$$,
  $$values ('guest-00000000-0000-4000-8000-0000000001a2', 'guest')$$,
  'someone not on the list who was let in gets a guest pass');
select lives_ok($$select public.leave_call(current_setting('t.call')::uuid)$$, 'and can leave');

-- ...and answers a ring from an invited contact.
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a1", "role": "authenticated"}';
select is((select call_id from public.ring('00000000-0000-4000-8000-0000000001a2')), current_setting('t.call')::uuid,
  'an invited host rings into their call');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a2", "role": "authenticated"}';
select is(jsonb_array_length(public.inbox() -> 'rings'), 1, 'someone not on the list sees the ring');
select is(public.answer_ring(current_setting('t.call')::uuid, true), 'admitted',
  'someone not on the list answers a ring');
select is((select role from public.pass_claims(current_setting('t.call')::uuid)), 'guest',
  '...and gets a guest pass');

-- Asking to host: once, however often it is called.
select lives_ok($$select public.ask_to_host()$$, 'someone not on the list asks to host');
reset role;
select ok(set_config('t.asked', (select asked_at::text from public.host_requests
                                  where user_id = '00000000-0000-4000-8000-0000000001a2'), true) is not null,
  'the request is recorded');
update public.host_requests set asked_at = asked_at - interval '1 hour';
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a2", "role": "authenticated"}';
select lives_ok($$select public.ask_to_host()$$, 'asking again works');
reset role;
select results_eq(
  $$select count(*)::int, min(asked_at) from public.host_requests$$,
  $$select 1, current_setting('t.asked')::timestamptz - interval '1 hour'$$,
  'asking again adds nothing and keeps the first time');
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a2", "role": "authenticated"}';
select results_eq(
  $$select (public.inbox() -> 'me' ->> 'can_host')::boolean, (public.inbox() -> 'me' ->> 'asked_to_host')::boolean$$,
  $$values (false, true)$$,
  'inbox says they asked, and still cannot host');

-- Kim has no GitHub handle, so can never be on the list.
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a3", "role": "authenticated"}';
select is((select handle from public.profiles where id = '00000000-0000-4000-8000-0000000001a3'), null,
  'a handle in editable metadata gives no handle');
select is((public.inbox() -> 'me' ->> 'can_host')::boolean, false, 'invite-only: someone without a handle cannot host');
select throws_ok($$select * from public.start_call()$$, '42501', 'not_invited: this server is invite-only',
  'invite-only: someone without a handle cannot start a call');
reset role;
select throws_ok($$insert into public.invited (handle) values (null)$$, '23502', null,
  'there is no way to invite an empty handle');

-- Pat was invited before signing up: can host at once.
insert into auth.users (id, email, raw_user_meta_data) values
  ('00000000-0000-4000-8000-0000000001a4', 'pat@example.test', '{}');
insert into auth.identities (provider_id, user_id, identity_data, provider) values
  ('2004', '00000000-0000-4000-8000-0000000001a4', '{"sub": "2004", "user_name": "Pat-Example", "name": "Pat"}', 'github');
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a4", "role": "authenticated"}';
select is((public.inbox() -> 'me' ->> 'can_host')::boolean, true, 'someone invited before signing up can host at once');
select ok(s.call_id is not null, '...and starts a call') from public.start_call() s;
reset role;

-- The waiting list: everyone not on the list, those who asked first.
select results_eq(
  $$select handle, name, asked_to_host from public.waiting_to_host$$,
  $$values ('sam-example'::text, 'Sam'::text, true), (null, 'Kim', false)$$,
  'the waiting list shows who is not invited, who asked first, and nobody on the list');
select ok(
  (select signed_up_at is not null and asked_at is not null from public.waiting_to_host where handle = 'sam-example'),
  'the waiting list says when they signed up and when they asked');

-- Inviting Sam takes them off the waiting list and lets them host.
insert into public.invited (handle) values ('Sam-Example');
select is((select count(*)::int from public.waiting_to_host where handle = 'sam-example'), 0,
  'once invited, they leave the waiting list');
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a2", "role": "authenticated"}';
select is((public.inbox() -> 'me' ->> 'can_host')::boolean, true, 'once invited, they can host');
reset role;
delete from public.invited where handle = 'sam-example';

-- A GitHub rename changes the handle: the invite was for the old one.
update auth.identities set identity_data = identity_data || '{"user_name": "pat-renamed"}'
where user_id = '00000000-0000-4000-8000-0000000001a4';
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a4", "role": "authenticated"}';
select is((public.inbox() -> 'me' ->> 'can_host')::boolean, false, 'after a GitHub rename the old invite no longer counts');
reset role;

-------------------------------------------------------------------------------------------------
-- A host removed from the list mid-call: that call goes on until it ends; no new one starts
-------------------------------------------------------------------------------------------------

set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a5", "role": "authenticated"}';
select ok(set_config('t.lee', s.call_id::text, true) is not null and set_config('t.lee_code', s.code, true) is not null,
          'Lee, invited, starts a call')
from public.start_call() s;
reset role;
delete from public.invited where handle = 'lee-example';
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a5", "role": "authenticated"}';
select is((public.inbox(current_setting('t.lee')::uuid) -> 'hosting' ->> 'call_id')::uuid, current_setting('t.lee')::uuid,
  'removed mid-call: the heartbeat still keeps the live call going');
select is((public.inbox() -> 'me' ->> 'can_host')::boolean, false, 'removed mid-call: inbox says they can no longer host');
select results_eq(
  $$select identity, role from public.pass_claims(current_setting('t.lee')::uuid)$$,
  $$values ('sharer-00000000-0000-4000-8000-0000000001a5', 'sharer')$$,
  'removed mid-call: a sharer pass for the live call still works (an engine restart keeps the call)');
select throws_ok($$select * from public.start_call()$$, '42501', 'not_invited: this server is invite-only',
  'removed mid-call: no new call can be started');
select throws_ok($$select * from public.ring('00000000-0000-4000-8000-0000000001a1')$$, '42501',
  'not_invited: this server is invite-only', 'removed mid-call: ringing is refused, even into the live call');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a3", "role": "authenticated"}';
select is((select state from public.knock(current_setting('t.lee_code'))), 'knocking',
  'removed mid-call: someone can still knock on the live call');
reset role;
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a5", "role": "authenticated"}';
select is(public.answer_knock(current_setting('t.lee')::uuid, '00000000-0000-4000-8000-0000000001a3', true), 'admitted',
  'removed mid-call: the host can still let them in');
select lives_ok($$select public.end_call(current_setting('t.lee')::uuid)$$, 'the removed host ends the call');
select throws_ok($$select * from public.pass_claims(current_setting('t.lee')::uuid)$$, '42501', 'not_allowed',
  'once it has ended, there is no pass for it');
select throws_ok($$select * from public.start_call()$$, '42501', 'not_invited: this server is invite-only',
  'and still no new call');
reset role;

-------------------------------------------------------------------------------------------------
-- Back to open
-------------------------------------------------------------------------------------------------

select lives_ok($$update public.server_settings set invite_only = false$$, 'invite_only turns off');
select lives_ok($$delete from public.invited$$, 'with it off, the whole list can be removed');
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000001a2", "role": "authenticated"}';
select is((public.inbox() -> 'me' ->> 'can_host')::boolean, true, 'open again: everyone can host');
select ok(s.call_id is not null, 'open again: someone never invited starts a call') from public.start_call() s;
reset role;

select * from finish();
rollback;
