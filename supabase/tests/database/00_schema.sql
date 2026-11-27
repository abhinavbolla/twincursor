-- Privileges, row-level security and the profile trigger.
begin;
create extension if not exists pgtap with schema extensions;
select plan(29);

-- Row-level security is on everywhere, and nothing but reads has a policy.
select ok(
  (select bool_and(c.relrowsecurity) from pg_class c
   where c.oid in ('public.profiles'::regclass, 'public.calls'::regclass,
                   'public.call_members'::regclass, 'public.contacts'::regclass)),
  'row-level security is on for every table');
select is(
  (select count(*)::int from pg_policies where schemaname = 'public' and cmd <> 'SELECT'),
  0, 'there are no insert, update or delete policies');

select is(
  (select count(*)::int
   from unnest(array['public.profiles', 'public.calls', 'public.call_members', 'public.contacts']) t(name),
        unnest(array['INSERT', 'UPDATE', 'DELETE', 'TRUNCATE', 'REFERENCES', 'TRIGGER']) p(priv),
        unnest(array['anon', 'authenticated']) r(role)
   where has_table_privilege(r.role, t.name, p.priv)),
  0, 'anon and authenticated hold no write privilege on any table');
select is(
  (select count(*)::int
   from unnest(array['public.profiles', 'public.calls', 'public.call_members', 'public.contacts']) t(name)
   where has_table_privilege('anon', t.name, 'SELECT')),
  0, 'anon can read no table');

-- Functions: anon executes nothing of ours; authenticated executes exactly the eleven RPCs.
select is(
  (select count(*)::int
   from pg_proc f join pg_namespace n on n.oid = f.pronamespace
   where n.nspname in ('public', 'private')
     and not exists (select 1 from pg_depend d where d.objid = f.oid and d.deptype = 'e')
     and has_function_privilege('anon', f.oid, 'EXECUTE')),
  0, 'anon can execute none of the functions');
select is(
  (select array_agg(f.proname::text order by f.proname::text)
   from pg_proc f join pg_namespace n on n.oid = f.pronamespace
   where n.nspname = 'public'
     and not exists (select 1 from pg_depend d where d.objid = f.oid and d.deptype = 'e')
     and has_function_privilege('authenticated', f.oid, 'EXECUTE')),
  array['answer_knock', 'answer_ring', 'ask_to_host', 'end_call', 'inbox', 'knock', 'leave_call',
        'pass_claims', 'remove_contact', 'ring', 'start_call'],
  'authenticated can execute the eleven RPC functions and nothing else in public');
select is(
  (select count(*)::int
   from pg_proc f join pg_namespace n on n.oid = f.pronamespace
   where n.nspname = 'private' and has_function_privilege('authenticated', f.oid, 'EXECUTE')
     and f.proname not in ('is_live', 'is_host', 'is_member', 'shares_live_call', 'is_contact')),
  0, 'authenticated can execute only the read-only policy helpers in private');
select is(
  (select count(*)::int
   from pg_proc f join pg_namespace n on n.oid = f.pronamespace
   where n.nspname in ('public', 'private')
     and not exists (select 1 from pg_depend d where d.objid = f.oid and d.deptype = 'e')
     and f.prosecdef
     and not coalesce('search_path=""' = any(f.proconfig), false)),
  0, 'every security definer function pins an empty search_path');

-- Users for the rest of this file.
insert into auth.users (id, email, raw_user_meta_data) values
  ('00000000-0000-4000-8000-0000000000a1', 'ada@example.test',
   '{"name": "Ada Lovelace", "user_name": "ada", "avatar_url": "https://avatars.example.test/ada"}'),
  ('00000000-0000-4000-8000-0000000000a2', 'grace@example.test', '{"name": "Typed In"}'),
  ('00000000-0000-4000-8000-0000000000a3', 'linus@example.test',
   jsonb_build_object('full_name', E'Li\tnus\u0007 ' || repeat('x', 100))),
  ('00000000-0000-4000-8000-0000000000a4', null, '{}'),
  ('00000000-0000-4000-8000-0000000000a5', 'spoof@example.test',
   '{"name": "Not Torvalds", "user_name": "torvalds", "avatar_url": "https://evil.example.test/x.png"}'),
  ('00000000-0000-4000-8000-0000000000a6', 'ctl@example.test', jsonb_build_object('name', E'\n\t\u0001 '));

-- GitHub identities, as Supabase Auth stores them.
insert into auth.identities (provider_id, user_id, identity_data, provider) values
  ('1001', '00000000-0000-4000-8000-0000000000a1',
   '{"sub": "1001", "user_name": "ada", "name": "Ada Lovelace", "avatar_url": "https://avatars.githubusercontent.com/u/1001?v=4"}',
   'github'),
  ('1002', '00000000-0000-4000-8000-0000000000a2',
   '{"sub": "1002", "user_name": "grace-h", "avatar_url": "https://avatars.githubusercontent.com/u/1002?v=4"}',
   'github');

select results_eq(
  $$select name, handle, avatar_url from public.profiles where id = '00000000-0000-4000-8000-0000000000a1'$$,
  $$values ('Ada Lovelace', 'ada', 'https://avatars.githubusercontent.com/u/1001?v=4')$$,
  'GitHub sign-in gives a name, handle and avatar');
select results_eq(
  $$select name, handle from public.profiles where id = '00000000-0000-4000-8000-0000000000a2'$$,
  $$values ('grace-h', 'grace-h')$$,
  'without a GitHub name, the GitHub handle is the name, whatever the metadata says');
select is(
  (select name from public.profiles where id = '00000000-0000-4000-8000-0000000000a3'),
  'Linus ' || repeat('x', 58),
  'control characters are removed and the name is cut to 64 characters');
select is(
  (select char_length(name) from public.profiles where id = '00000000-0000-4000-8000-0000000000a3'),
  64, 'a long name is exactly 64 characters');
select is(
  (select name from public.profiles where id = '00000000-0000-4000-8000-0000000000a4'),
  'Someone', 'with nothing to go on, the name is "Someone"');
select results_eq(
  $$select name, handle, avatar_url from public.profiles where id = '00000000-0000-4000-8000-0000000000a5'$$,
  $$values ('Not Torvalds', null::text, null::text)$$,
  'a handle or avatar in editable metadata is ignored without a GitHub identity');
select is(
  (select name from public.profiles where id = '00000000-0000-4000-8000-0000000000a6'),
  'ctl', 'a name of only control characters falls back to the email''s local part');

-- What updateUser({ data }) does: a signed-in user rewrites their own metadata.
update auth.users set raw_user_meta_data = '{"name": "Linus Torvalds", "full_name": "Linus Torvalds"}'
where id = '00000000-0000-4000-8000-0000000000a1';
select results_eq(
  $$select name, handle from public.profiles where id = '00000000-0000-4000-8000-0000000000a1'$$,
  $$values ('Ada Lovelace', 'ada')$$,
  'a GitHub user who edits their own metadata keeps their GitHub name');

update auth.users set raw_user_meta_data = '{"name": "Renamed Spoof"}'
where id = '00000000-0000-4000-8000-0000000000a5';
select is(
  (select name from public.profiles where id = '00000000-0000-4000-8000-0000000000a5'),
  'Renamed Spoof', 'without a GitHub identity, an edited metadata name is used');

update auth.identities
set identity_data = identity_data || jsonb_build_object('user_name', 'ada-l', 'name', E'Ada\u0007 King ' || repeat('y', 80))
where user_id = '00000000-0000-4000-8000-0000000000a1';
select results_eq(
  $$select name, handle from public.profiles where id = '00000000-0000-4000-8000-0000000000a1'$$,
  $$select 'Ada King ' || repeat('y', 55), 'ada-l'$$,
  'when the GitHub identity changes, the name (cleaned) and handle follow');

update auth.identities set identity_data = (identity_data - 'name') || '{"full_name": "Ada Full"}'
where user_id = '00000000-0000-4000-8000-0000000000a1';
select is(
  (select name from public.profiles where id = '00000000-0000-4000-8000-0000000000a1'),
  'Ada Full', 'a GitHub full_name is used when there is no name');

delete from auth.users where id in ('00000000-0000-4000-8000-0000000000a4', '00000000-0000-4000-8000-0000000000a2');
select is(
  (select count(*)::int from public.profiles
   where id in ('00000000-0000-4000-8000-0000000000a4', '00000000-0000-4000-8000-0000000000a2')),
  0, 'deleting an account, with or without a GitHub identity, deletes its profile');

-- Direct writes fail, whoever is signed in.
set local role authenticated;
set local request.jwt.claims to '{"sub": "00000000-0000-4000-8000-0000000000a1", "role": "authenticated"}';

select throws_ok(
  $$insert into public.profiles (id, name) values ('00000000-0000-4000-8000-0000000000a1', 'x')$$,
  '42501', null, 'authenticated cannot insert a profile');
select throws_ok(
  $$update public.profiles set name = 'x' where id = '00000000-0000-4000-8000-0000000000a1'$$,
  '42501', null, 'authenticated cannot update their own profile directly');
select throws_ok(
  $$insert into public.calls (host) values ('00000000-0000-4000-8000-0000000000a1')$$,
  '42501', null, 'authenticated cannot insert a call');
select throws_ok(
  $$update public.call_members set state = 'admitted'$$,
  '42501', null, 'authenticated cannot update a member row');
select throws_ok(
  $$insert into public.contacts (owner, contact)
    values ('00000000-0000-4000-8000-0000000000a1', '00000000-0000-4000-8000-0000000000a2')$$,
  '42501', null, 'authenticated cannot add a contact directly');
select throws_ok(
  $$delete from public.contacts$$,
  '42501', null, 'authenticated cannot delete contacts directly');
select is(
  (select count(*)::int from public.profiles),
  1, 'a signed-in user with no contacts or calls sees only their own profile');

reset role;
set local role anon;
set local request.jwt.claims to '{"role": "anon"}';
select throws_ok($$select * from public.profiles$$, '42501', null, 'anon cannot read profiles');
select throws_ok($$select public.inbox()$$, '42501', null, 'anon cannot call inbox()');

select * from finish();
rollback;
