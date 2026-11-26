-- Accounts, contacts and calls.
--
-- Clients read through `inbox()` and change state only through the RPC functions below. Every
-- table has row-level security on and no insert, update or delete policy, so a client can never
-- write a row directly. `pass_claims()` is the one place that decides who may join a call's media
-- room; the `call-pass` Edge Function turns its answer into a LiveKit token.
--
-- Errors raised by the functions carry a stable message (the first word, e.g. `call_not_found`)
-- and SQLSTATE, listed in supabase/README.md.

create extension if not exists pgcrypto with schema extensions;

-- Helpers that policies and functions share. Not exposed through the API.
create schema private;
comment on schema private is 'Internal helpers for row-level security and the RPC functions. Not exposed through the API.';
revoke all on schema private from public;
grant usage on schema private to authenticated;

-------------------------------------------------------------------------------------------------
-- Tables
-------------------------------------------------------------------------------------------------

create table public.profiles (
  id uuid primary key references auth.users (id) on delete cascade,
  name text not null
    check (char_length(name) between 1 and 64 and name !~ '[\u0001-\u001f\u007f-\u009f]'),
  handle text check (handle is null or (char_length(handle) between 1 and 64 and handle ~ '^[A-Za-z0-9-]+$')),
  avatar_url text check (avatar_url is null or (char_length(avatar_url) <= 2048 and avatar_url ~ '^https://')),
  created_at timestamptz not null default now()
);
comment on table public.profiles is 'One row per account, kept in sync with auth.users and the GitHub identity by triggers. name is what the person calls themselves on GitHub (not verified by anyone); handle is their GitHub login. With a GitHub identity all three come from it and can''t be edited by the user; only accounts without one take name from user metadata.';

create table public.calls (
  id uuid primary key default gen_random_uuid(),
  host uuid not null references public.profiles (id) on delete cascade,
  room text not null unique default encode(extensions.gen_random_bytes(16), 'hex'),
  code text not null unique default encode(extensions.gen_random_bytes(16), 'hex'),
  created_at timestamptz not null default now(),
  seen_at timestamptz not null default now(),
  ended_at timestamptz,
  check (room ~ '^[0-9a-f]{32}$' and code ~ '^[0-9a-f]{32}$' and room <> code)
);
comment on table public.calls is 'A call hosted (shared) by one person. Live while ended_at is null and the host was seen in the last 45 s.';
comment on column public.calls.room is 'LiveKit room name: 128 random bits as 32 lowercase hex characters. Only ever handed out inside a call pass.';
comment on column public.calls.code is 'What a call link carries: 128 random bits as 32 lowercase hex characters, independent of room.';
comment on column public.calls.seen_at is 'Host heartbeat, refreshed by inbox(hosting => id) while the host''s app is sharing this call.';

-- A host has at most one call that isn't ended; start_call() ends the earlier one.
create unique index calls_one_open_per_host on public.calls (host) where ended_at is null;

create table public.call_members (
  call_id uuid not null references public.calls (id) on delete cascade,
  user_id uuid not null references public.profiles (id) on delete cascade,
  state text not null check (state in ('ringing', 'knocking', 'admitted', 'turned_away', 'declined', 'left')),
  created_at timestamptz not null default now(),
  changed_at timestamptz not null default now(),
  answered_at timestamptz,
  primary key (call_id, user_id)
);
comment on table public.call_members is 'Everyone other than the host who knocked on or was rung into a call, and where they stand.';
comment on column public.call_members.changed_at is 'When state last changed. A ring is pending for 60 s from here.';
comment on column public.call_members.answered_at is 'When the knock or ring was answered (admitted, turned_away or declined).';

create index call_members_user on public.call_members (user_id, state);

create table public.contacts (
  owner uuid not null references public.profiles (id) on delete cascade,
  contact uuid not null references public.profiles (id) on delete cascade,
  created_at timestamptz not null default now(),
  primary key (owner, contact),
  check (owner <> contact)
);
comment on table public.contacts is 'People you have been in a call with. Admitting a knock adds both directions; ringing needs both.';

create index contacts_contact on public.contacts (contact);

-------------------------------------------------------------------------------------------------
-- Privileges: clients may only read (through RLS) and call the functions.
-------------------------------------------------------------------------------------------------

revoke all on public.profiles, public.calls, public.call_members, public.contacts from public, anon, authenticated;
grant select on public.profiles, public.calls, public.call_members, public.contacts to authenticated;

alter table public.profiles enable row level security;
alter table public.calls enable row level security;
alter table public.call_members enable row level security;
alter table public.contacts enable row level security;

-------------------------------------------------------------------------------------------------
-- Private helpers
-------------------------------------------------------------------------------------------------

-- The signed-in user, or an error.
create function private.me()
returns uuid
language plpgsql
stable
set search_path = ''
as $$
declare
  uid uuid := auth.uid();
begin
  if uid is null then
    raise exception 'not_signed_in' using errcode = '42501';
  end if;
  return uid;
end;
$$;
comment on function private.me() is 'auth.uid(), or raises not_signed_in.';

create function private.is_live(p_ended_at timestamptz, p_seen_at timestamptz)
returns boolean
language sql
stable
set search_path = ''
as $$
  select p_ended_at is null and p_seen_at > now() - interval '45 seconds';
$$;
comment on function private.is_live(timestamptz, timestamptz) is 'A call is live while it isn''t ended and its host was seen in the last 45 s.';

-- Removes control characters (Unicode Cc, as Rust's char::is_control), trims, caps the length.
create function private.clean_text(p_text text, p_max int)
returns text
language sql
immutable
set search_path = ''
as $$
  select nullif(
    regexp_replace(
      left(regexp_replace(regexp_replace(p_text, '[\u0001-\u001f\u007f-\u009f]', '', 'g'), '^\s+', ''), p_max),
      '\s+$', ''),
    '');
$$;
comment on function private.clean_text(text, int) is 'Strips control characters and surrounding white space and caps the length; null if nothing is left.';

create function private.is_host(p_call uuid)
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
  select exists (select 1 from public.calls c where c.id = p_call and c.host = auth.uid());
$$;
comment on function private.is_host(uuid) is 'RLS helper: the signed-in user hosts this call.';

create function private.is_member(p_call uuid)
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
  select exists (select 1 from public.call_members m where m.call_id = p_call and m.user_id = auth.uid());
$$;
comment on function private.is_member(uuid) is 'RLS helper: the signed-in user has a member row in this call.';

create function private.shares_live_call(p_other uuid)
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
  select exists (
    select 1
    from public.calls c
    join public.call_members m on m.call_id = c.id
    where private.is_live(c.ended_at, c.seen_at)
      and m.state not in ('turned_away', 'declined')
      and ((c.host = auth.uid() and m.user_id = p_other)
        or (c.host = p_other and m.user_id = auth.uid()))
  );
$$;
comment on function private.shares_live_call(uuid) is 'RLS helper: the signed-in user and p_other are host and member of a live call (not turned away or declined).';

create function private.is_contact(p_other uuid)
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
  select exists (select 1 from public.contacts k where k.owner = auth.uid() and k.contact = p_other);
$$;
comment on function private.is_contact(uuid) is 'RLS helper: p_other is in the signed-in user''s contacts.';

create function private.profile_json(p_id uuid)
returns jsonb
language sql
stable
security definer
set search_path = ''
as $$
  select jsonb_build_object('id', p.id, 'name', p.name, 'handle', p.handle, 'avatar_url', p.avatar_url)
  from public.profiles p where p.id = p_id;
$$;
comment on function private.profile_json(uuid) is 'A profile as the JSON object inbox() uses.';

-- Ends the user's open call, if any, and starts a new one.
create function private.open_call(p_host uuid, out call_id uuid, out code text)
language plpgsql
security definer
set search_path = ''
as $$
begin
  update public.calls c set ended_at = now() where c.host = p_host and c.ended_at is null;
  insert into public.calls (host) values (p_host) returning calls.id, calls.code into call_id, code;
end;
$$;
comment on function private.open_call(uuid) is 'Ends p_host''s open call, if any, and starts a new one. Callers lock the host''s profile row first.';

-- Locks the caller's profile row, so one person's call changes run one at a time.
create function private.lock_me()
returns uuid
language plpgsql
security definer
set search_path = ''
as $$
declare
  uid uuid := private.me();
begin
  perform 1 from public.profiles p where p.id = uid for update;
  if not found then
    raise exception 'no_profile' using errcode = '42501';
  end if;
  return uid;
end;
$$;
comment on function private.lock_me() is 'Locks and returns the signed-in user''s profile row, so their call changes are serialized.';

revoke all on all functions in schema private from public, anon, authenticated;
grant execute on function
  private.is_live(timestamptz, timestamptz),
  private.is_host(uuid),
  private.is_member(uuid),
  private.shares_live_call(uuid),
  private.is_contact(uuid)
to authenticated;

-------------------------------------------------------------------------------------------------
-- Read policies
-------------------------------------------------------------------------------------------------

create policy "profiles: self, contacts and people in a live call with you"
  on public.profiles for select to authenticated
  using (
    id = (select auth.uid())
    or private.is_contact(id)
    or private.shares_live_call(id)
  );

create policy "calls: host or member"
  on public.calls for select to authenticated
  using (host = (select auth.uid()) or private.is_member(id));

create policy "call_members: the call's host, or your own row"
  on public.call_members for select to authenticated
  using (user_id = (select auth.uid()) or private.is_host(call_id));

create policy "contacts: owner"
  on public.contacts for select to authenticated
  using (owner = (select auth.uid()));

-------------------------------------------------------------------------------------------------
-- Profiles from auth
-------------------------------------------------------------------------------------------------

-- For an account with a GitHub identity, everything comes from that identity, which the user can't
-- edit: name is what they call themselves on GitHub, handle is their GitHub login. User metadata
-- (raw_user_meta_data) can be rewritten by the signed-in user with updateUser, so it is used only
-- for accounts without a GitHub identity.
create function private.sync_profile(p_user uuid)
returns void
language plpgsql
security definer
set search_path = ''
as $$
declare
  meta jsonb;
  v_email text;
  gh jsonb;
  v_handle text;
  v_avatar text;
  v_name text;
begin
  select u.raw_user_meta_data, u.email into meta, v_email from auth.users u where u.id = p_user;
  if not found then
    return;
  end if;
  meta := coalesce(meta, '{}'::jsonb);

  select i.identity_data into gh
  from auth.identities i
  where i.user_id = p_user and i.provider = 'github'
  order by i.created_at
  limit 1;

  v_handle := gh ->> 'user_name';
  if v_handle is null or v_handle !~ '^[A-Za-z0-9-]{1,64}$' then
    v_handle := null;
  end if;

  v_avatar := gh ->> 'avatar_url';
  if v_avatar is null or v_avatar !~ '^https://' or char_length(v_avatar) > 2048 then
    v_avatar := null;
  end if;

  if gh is not null then
    v_name := coalesce(
      private.clean_text(gh ->> 'name', 64),
      private.clean_text(gh ->> 'full_name', 64),
      v_handle,
      private.clean_text(gh ->> 'user_name', 64));
  else
    v_name := coalesce(
      private.clean_text(meta ->> 'name', 64),
      private.clean_text(meta ->> 'full_name', 64),
      private.clean_text(meta ->> 'user_name', 64));
  end if;
  v_name := coalesce(v_name, private.clean_text(split_part(v_email, '@', 1), 64), 'Someone');

  insert into public.profiles (id, name, handle, avatar_url)
  values (p_user, v_name, v_handle, v_avatar)
  on conflict (id) do update
    set name = excluded.name, handle = excluded.handle, avatar_url = excluded.avatar_url;
end;
$$;
comment on function private.sync_profile(uuid) is 'Creates or refreshes a profile: from the GitHub identity when there is one, otherwise from auth.users metadata.';

create function private.on_auth_user()
returns trigger
language plpgsql
security definer
set search_path = ''
as $$
begin
  perform private.sync_profile(new.id);
  return null;
end;
$$;

create function private.on_auth_identity()
returns trigger
language plpgsql
security definer
set search_path = ''
as $$
begin
  if tg_op = 'DELETE' then
    perform private.sync_profile(old.user_id);
  else
    perform private.sync_profile(new.user_id);
  end if;
  return null;
end;
$$;

revoke all on function private.sync_profile(uuid), private.on_auth_user(), private.on_auth_identity()
  from public, anon, authenticated;

create trigger sync_profile_on_user
  after insert or update of raw_user_meta_data, email on auth.users
  for each row execute function private.on_auth_user();

create trigger sync_profile_on_identity
  after insert or update of identity_data or delete on auth.identities
  for each row execute function private.on_auth_identity();

-------------------------------------------------------------------------------------------------
-- RPC functions
-------------------------------------------------------------------------------------------------

create function public.start_call()
returns table (call_id uuid, code text)
language plpgsql
security definer
set search_path = ''
as $$
declare
  uid uuid := private.lock_me();
begin
  return query select o.call_id, o.code from private.open_call(uid) o;
end;
$$;
comment on function public.start_call() is 'Starts a call hosted by the caller and returns its id and code. Ends the caller''s earlier call, if any.';

create function public.knock(code text)
returns table (call_id uuid, state text, host_id uuid, host_name text, host_handle text, host_avatar_url text)
language plpgsql
security definer
set search_path = ''
as $$
declare
  uid uuid := private.me();
  c public.calls;
  v_state text;
begin
  select * into c from public.calls k where k.code = knock.code for share;
  if not found or not private.is_live(c.ended_at, c.seen_at) then
    raise exception 'call_not_found' using errcode = 'P0001';
  end if;
  if c.host = uid then
    raise exception 'own_call: you host this call' using errcode = '22023';
  end if;

  insert into public.call_members as m (call_id, user_id, state)
  values (c.id, uid, 'knocking')
  on conflict on constraint call_members_pkey do update
    set state = 'knocking', changed_at = now(), answered_at = null
    where m.state = 'left'
  returning m.state into v_state;

  if v_state is null then
    select m.state into v_state from public.call_members m where m.call_id = c.id and m.user_id = uid;
  end if;

  return query
    select c.id, v_state, p.id, p.name, p.handle, p.avatar_url
    from public.profiles p where p.id = c.host;
end;
$$;
comment on function public.knock(text) is 'Knocks on the live call with this code and returns where the caller stands. Repeating it returns the same state; only someone who left knocks again. Unknown and dead codes both raise call_not_found.';

create function public.answer_knock(call_id uuid, user_id uuid, let_in boolean)
returns text
language plpgsql
security definer
set search_path = ''
as $$
declare
  uid uuid := private.me();
  c public.calls;
  v_state text;
begin
  if let_in is null then
    raise exception 'bad_argument: let_in is required' using errcode = '22023';
  end if;
  select * into c from public.calls k where k.id = answer_knock.call_id and k.host = uid for update;
  if not found or not private.is_live(c.ended_at, c.seen_at) then
    raise exception 'call_not_found' using errcode = 'P0001';
  end if;

  update public.call_members m
    set state = case when let_in then 'admitted' else 'turned_away' end,
        changed_at = now(),
        answered_at = now()
    where m.call_id = c.id and m.user_id = answer_knock.user_id and m.state = 'knocking'
    returning m.state into v_state;
  if v_state is null then
    raise exception 'not_knocking: nobody is waiting with that id' using errcode = 'P0001';
  end if;

  if let_in then
    insert into public.contacts (owner, contact)
    values (uid, answer_knock.user_id), (answer_knock.user_id, uid)
    on conflict do nothing;
  end if;
  return v_state;
end;
$$;
comment on function public.answer_knock(uuid, uuid, boolean) is 'Host only: lets a knocking person in (admitted, and both become contacts) or turns them away. Works once per knock.';

create function public.ring(contact uuid)
returns table (call_id uuid, code text)
language plpgsql
security definer
set search_path = ''
as $$
declare
  uid uuid := private.lock_me();
  c public.calls;
  v_call uuid;
begin
  if not exists (select 1 from public.contacts k where k.owner = uid and k.contact = ring.contact)
     or not exists (select 1 from public.contacts k where k.owner = ring.contact and k.contact = uid) then
    raise exception 'not_a_contact' using errcode = '42501';
  end if;

  -- Ringing is the host acting, so it counts as a heartbeat for their open call, stale or not: the
  -- app may still be sharing it after a missed poll. Only without an open call is one started.
  update public.calls k set seen_at = now()
  where k.host = uid and k.ended_at is null
  returning * into c;
  if c.id is null then
    select o.call_id into v_call from private.open_call(uid) o;
    select * into c from public.calls k where k.id = v_call;
  end if;

  insert into public.call_members as m (call_id, user_id, state)
  values (c.id, ring.contact, 'ringing')
  on conflict on constraint call_members_pkey do update
    set state = 'ringing', changed_at = now(), answered_at = null
    where m.state in ('ringing', 'declined', 'left', 'turned_away');

  return query select c.id, c.code;
end;
$$;
comment on function public.ring(uuid) is 'Rings a contact (who must have the caller as a contact too) into the caller''s open call (refreshing its heartbeat, so a stale one comes back), starting one if there is none. Ringing again restarts the 60 s; someone admitted or knocking is left as they are.';

create function public.answer_ring(call_id uuid, accept boolean)
returns text
language plpgsql
security definer
set search_path = ''
as $$
declare
  uid uuid := private.me();
  v_state text;
begin
  if accept is null then
    raise exception 'bad_argument: accept is required' using errcode = '22023';
  end if;
  perform 1 from public.calls k where k.id = answer_ring.call_id for share;

  update public.call_members m
    set state = case when accept then 'admitted' else 'declined' end,
        changed_at = now(),
        answered_at = now()
    from public.calls c
    where m.call_id = answer_ring.call_id
      and m.user_id = uid
      and m.state = 'ringing'
      and m.changed_at > now() - interval '60 seconds'
      and c.id = m.call_id
      and private.is_live(c.ended_at, c.seen_at)
    returning m.state into v_state;
  if v_state is null then
    raise exception 'no_pending_ring' using errcode = 'P0001';
  end if;
  return v_state;
end;
$$;
comment on function public.answer_ring(uuid, boolean) is 'The person being rung joins (admitted) or declines, within 60 s of the ring and while the call is live.';

create function public.leave_call(call_id uuid)
returns void
language plpgsql
security definer
set search_path = ''
as $$
declare
  uid uuid := private.me();
  v_state text;
begin
  select m.state into v_state from public.call_members m
  where m.call_id = leave_call.call_id and m.user_id = uid for update;
  if v_state = 'left' then
    return;
  end if;
  if v_state is null or v_state not in ('knocking', 'admitted') then
    raise exception 'not_in_call' using errcode = 'P0001';
  end if;
  update public.call_members m set state = 'left', changed_at = now()
  where m.call_id = leave_call.call_id and m.user_id = uid;
end;
$$;
comment on function public.leave_call(uuid) is 'A member leaves a call they were admitted to, or withdraws a knock. Leaving twice is a no-op.';

create function public.end_call(call_id uuid)
returns void
language plpgsql
security definer
set search_path = ''
as $$
declare
  uid uuid := private.me();
begin
  update public.calls k set ended_at = coalesce(k.ended_at, now())
  where k.id = end_call.call_id and k.host = uid;
  if not found then
    raise exception 'call_not_found' using errcode = 'P0001';
  end if;
end;
$$;
comment on function public.end_call(uuid) is 'Host only: ends the call. Ending it again is a no-op.';

create function public.remove_contact(contact uuid)
returns void
language plpgsql
security definer
set search_path = ''
as $$
declare
  uid uuid := private.me();
begin
  delete from public.contacts k where k.owner = uid and k.contact = remove_contact.contact;
end;
$$;
comment on function public.remove_contact(uuid) is 'Removes someone from the caller''s contacts (only the caller''s side). They can no longer ring the caller.';

create function public.pass_claims(call_id uuid)
returns table (room text, identity text, name text, role text)
language plpgsql
stable
security definer
set search_path = ''
as $$
declare
  uid uuid := private.me();
  c public.calls;
  v_name text;
begin
  select * into c from public.calls k where k.id = pass_claims.call_id;
  if found and private.is_live(c.ended_at, c.seen_at) then
    select p.name into v_name from public.profiles p where p.id = uid;
    if c.host = uid then
      return query select c.room, 'sharer-' || uid::text, v_name, 'sharer'::text;
      return;
    end if;
    if exists (select 1 from public.call_members m
               where m.call_id = c.id and m.user_id = uid and m.state = 'admitted') then
      return query select c.room, 'guest-' || uid::text, v_name, 'guest'::text;
      return;
    end if;
  end if;
  raise exception 'not_allowed' using errcode = '42501';
end;
$$;
comment on function public.pass_claims(uuid) is 'Decides who may join a call''s media room: its host (role sharer) or an admitted member (role guest), while the call is live. Raises not_allowed for everyone else.';

create function public.inbox(hosting uuid default null)
returns jsonb
language plpgsql
security definer
set search_path = ''
as $$
declare
  uid uuid := private.me();
  hosted public.calls;
  v_hosting jsonb;
begin
  -- Host heartbeat, only for the call the app says it is sharing. This also revives a call that
  -- went stale (a missed poll or two), but never an ended one. A wrong or foreign id is ignored.
  if inbox.hosting is not null then
    update public.calls k set seen_at = now()
    where k.id = inbox.hosting and k.host = uid and k.ended_at is null;
  end if;

  select * into hosted from public.calls k where k.host = uid and k.ended_at is null;

  if hosted.id is not null and private.is_live(hosted.ended_at, hosted.seen_at) then
    v_hosting := jsonb_build_object(
      'call_id', hosted.id,
      'code', hosted.code,
      'members', coalesce((
        select jsonb_agg(jsonb_build_object(
            'user_id', p.id,
            'name', p.name,
            'handle', p.handle,
            'avatar_url', p.avatar_url,
            'state', case when m.state = 'ringing' and m.changed_at <= now() - interval '60 seconds'
                          then 'missed' else m.state end)
          order by m.created_at, p.id)
        from public.call_members m
        join public.profiles p on p.id = m.user_id
        where m.call_id = hosted.id), '[]'::jsonb));
  end if;

  return jsonb_build_object(
    'me', private.profile_json(uid),
    'contacts', coalesce((
      select jsonb_agg(private.profile_json(k.contact) order by p.name, p.id)
      from public.contacts k join public.profiles p on p.id = k.contact
      where k.owner = uid), '[]'::jsonb),
    'hosting', v_hosting,
    'rings', coalesce((
      select jsonb_agg(jsonb_build_object('call_id', c.id, 'host', private.profile_json(c.host))
          order by m.changed_at desc)
      from public.call_members m join public.calls c on c.id = m.call_id
      where m.user_id = uid
        and m.state = 'ringing'
        and m.changed_at > now() - interval '60 seconds'
        and private.is_live(c.ended_at, c.seen_at)), '[]'::jsonb),
    'waiting', coalesce((
      select jsonb_agg(jsonb_build_object('call_id', c.id, 'state', m.state, 'host', private.profile_json(c.host))
          order by m.changed_at desc)
      from public.call_members m join public.calls c on c.id = m.call_id
      where m.user_id = uid
        and private.is_live(c.ended_at, c.seen_at)
        and (m.state = 'knocking'
          or (m.state in ('admitted', 'turned_away') and m.answered_at > now() - interval '60 seconds'))),
      '[]'::jsonb)
  );
end;
$$;
comment on function public.inbox(uuid) is 'Everything the app polls for, as one JSON object (see supabase/README.md). With hosting => the id of the call the app is sharing, also the host''s heartbeat for that call. Call it with POST.';

revoke execute on function
  public.start_call(),
  public.knock(text),
  public.answer_knock(uuid, uuid, boolean),
  public.ring(uuid),
  public.answer_ring(uuid, boolean),
  public.leave_call(uuid),
  public.end_call(uuid),
  public.remove_contact(uuid),
  public.pass_claims(uuid),
  public.inbox(uuid)
from public, anon;

grant execute on function
  public.start_call(),
  public.knock(text),
  public.answer_knock(uuid, uuid, boolean),
  public.ring(uuid),
  public.answer_ring(uuid, boolean),
  public.leave_call(uuid),
  public.end_call(uuid),
  public.remove_contact(uuid),
  public.pass_claims(uuid),
  public.inbox(uuid)
to authenticated;
