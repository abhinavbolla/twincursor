-- Who may start calls: an invite list and one server setting.
--
-- Anyone who signs in may join a call they are let into or rung into. Starting a call
-- (`start_call`, and `ring`, which starts or uses the caller's own call) is for people whose
-- GitHub handle is on `invited`, unless the server is open (`server_settings.invite_only` false,
-- the default). The server decides; the app only shows what `inbox()` says.
--
-- Applying this changes nothing for a running server: it starts open. The owner adds their own
-- handle to `invited` first, then turns `invite_only` on (docs/self-hosting.md). Two checks make
-- locking everyone out hard to do by accident: `invite_only` can't be turned on while the list is
-- empty, and the last handle can't be removed while it is on.
--
-- Clients can't read or write any of the new tables or the view: row-level security is on, there
-- are no policies and no grants to `anon` or `authenticated`. The project owner manages them in
-- the dashboard's table editor or with SQL; the service role can too.

-------------------------------------------------------------------------------------------------
-- Tables
-------------------------------------------------------------------------------------------------

create table public.invited (
  handle text primary key check (handle ~ '^[a-z0-9-]{1,64}$'),
  note text check (note is null or char_length(note) <= 500),
  created_at timestamptz not null default now()
);
comment on table public.invited is 'GitHub handles that may start calls when server_settings.invite_only is on. Stored lowercased (a leading @ and surrounding spaces are dropped), compared case-insensitively. A handle can be added before that person ever signs in. Not readable by clients.';
comment on column public.invited.note is 'Who this is, for the owner. Optional.';

create table public.server_settings (
  id boolean primary key default true check (id),
  invite_only boolean not null default false,
  updated_at timestamptz not null default now()
);
comment on table public.server_settings is 'One row. invite_only: only handles on public.invited may start calls; everyone signed in may still join calls they are let into. Not readable by clients.';
comment on column public.server_settings.invite_only is 'Off: anyone signed in may start calls. On: only handles on public.invited. Add your own handle first; turning it on with an empty list is refused.';

insert into public.server_settings (id, invite_only) values (true, false);

create table public.host_requests (
  user_id uuid primary key references public.profiles (id) on delete cascade,
  asked_at timestamptz not null default now()
);
comment on table public.host_requests is 'People who asked to be able to start calls (ask_to_host). Shown to the owner in waiting_to_host. Not readable by clients.';

-------------------------------------------------------------------------------------------------
-- Privileges: nothing for clients. The owner (postgres) and the service role manage these.
-------------------------------------------------------------------------------------------------

revoke all on public.invited, public.server_settings, public.host_requests from public, anon, authenticated;
grant select, insert, update, delete on public.invited, public.server_settings, public.host_requests to service_role;

alter table public.invited enable row level security;
alter table public.server_settings enable row level security;
alter table public.host_requests enable row level security;

-------------------------------------------------------------------------------------------------
-- The rule
-------------------------------------------------------------------------------------------------

create function private.can_host(p_user uuid)
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
  select not coalesce((select s.invite_only from public.server_settings s where s.id), false)
    or exists (
      select 1
      from public.profiles p
      join public.invited i on i.handle = lower(p.handle)
      where p.id = p_user);
$$;
comment on function private.can_host(uuid) is 'p_user may start calls: the server is open, or their GitHub handle is on public.invited. An account without a handle is never invited.';

create function private.require_host(p_user uuid)
returns void
language plpgsql
stable
security definer
set search_path = ''
as $$
begin
  if not private.can_host(p_user) then
    raise exception 'not_invited: this server is invite-only' using errcode = '42501';
  end if;
end;
$$;
comment on function private.require_host(uuid) is 'Raises not_invited unless p_user may start calls.';

-------------------------------------------------------------------------------------------------
-- The owner's safety checks
-------------------------------------------------------------------------------------------------

-- Handles are stored the way GitHub compares them: lowercased, without a leading @ or spaces.
create function private.normalize_invited()
returns trigger
language plpgsql
set search_path = ''
as $$
begin
  new.handle := lower(regexp_replace(btrim(new.handle), '^@', ''));
  return new;
end;
$$;

create trigger invited_normalize
  before insert or update of handle on public.invited
  for each row execute function private.normalize_invited();

-- Refuses to turn invite_only on while the list is empty, and keeps the one row.
create function private.check_server_settings()
returns trigger
language plpgsql
set search_path = ''
as $$
begin
  if tg_op in ('DELETE', 'TRUNCATE') then
    raise exception 'keep_server_settings: this row holds the server''s settings; set invite_only to false instead of deleting it'
      using errcode = 'P0001';
  end if;
  if new.invite_only and not exists (select 1 from public.invited) then
    raise exception 'invite_list_empty: add your own GitHub handle to public.invited before turning invite_only on, or nobody could start a call'
      using errcode = 'P0001';
  end if;
  new.updated_at := now();
  return new;
end;
$$;

create trigger server_settings_check
  before insert or update or delete on public.server_settings
  for each row execute function private.check_server_settings();

create trigger server_settings_no_truncate
  before truncate on public.server_settings
  for each statement execute function private.check_server_settings();

-- Refuses to empty the list while invite_only is on. Runs after the whole statement, so deleting
-- several handles at once is judged by what is left. The settings row is locked, so this and
-- turning invite_only on can't race each other.
create function private.keep_invite_list()
returns trigger
language plpgsql
set search_path = ''
as $$
begin
  perform 1 from public.server_settings s where s.id and s.invite_only for update;
  if found and not exists (select 1 from public.invited) then
    raise exception 'last_invited: invite_only is on and this would empty the invite list, so nobody could start a call; turn invite_only off first'
      using errcode = 'P0001';
  end if;
  return null;
end;
$$;

create trigger invited_keep_one
  after delete on public.invited
  for each statement execute function private.keep_invite_list();

create trigger invited_keep_one_truncate
  after truncate on public.invited
  for each statement execute function private.keep_invite_list();

-------------------------------------------------------------------------------------------------
-- The waiting list, for the owner
-------------------------------------------------------------------------------------------------

-- security_invoker: the view reads with the reader's rights, so it can never show more than the
-- reader could read from the tables. Clients get no grant on it at all.
create view public.waiting_to_host
with (security_invoker = true)
as
  select
    p.handle,
    p.name,
    p.created_at as signed_up_at,
    r.user_id is not null as asked_to_host,
    r.asked_at,
    p.id as user_id
  from public.profiles p
  left join public.host_requests r on r.user_id = p.id
  where p.handle is null
     or not exists (select 1 from public.invited i where i.handle = lower(p.handle))
  order by r.asked_at desc nulls last, p.created_at desc;
comment on view public.waiting_to_host is 'Everyone signed in who is not on public.invited: who asked to start calls (asked_to_host) first, newest first. A row without a handle signed in without GitHub and can''t be invited. For the owner; clients can''t read it.';

revoke all on public.waiting_to_host from public, anon, authenticated;
grant select on public.waiting_to_host to service_role;

-------------------------------------------------------------------------------------------------
-- RPC functions
-------------------------------------------------------------------------------------------------

create or replace function public.start_call()
returns table (call_id uuid, code text)
language plpgsql
security definer
set search_path = ''
as $$
declare
  uid uuid := private.lock_me();
begin
  perform private.require_host(uid);
  return query select o.call_id, o.code from private.open_call(uid) o;
end;
$$;
comment on function public.start_call() is 'Starts a call hosted by the caller and returns its id and code. Ends the caller''s earlier call, if any. Raises not_invited on an invite-only server for someone not on the invite list.';

create or replace function public.ring(contact uuid)
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
  perform private.require_host(uid);
  if not exists (select 1 from public.contacts k where k.owner = uid and k.contact = ring.contact)
     or not exists (select 1 from public.contacts k where k.owner = ring.contact and k.contact = uid) then
    raise exception 'not_a_contact' using errcode = '42501';
  end if;

  -- Ringing is the host acting, so it counts as a heartbeat for their open call, even a stale one:
  -- the app may still be sharing it after a missed poll. A call silent for over 10 minutes is over,
  -- though: open_call ends it and starts a new one, so its old members don't come back with it.
  update public.calls k set seen_at = now()
  where k.host = uid and private.can_revive(k.ended_at, k.seen_at)
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
comment on function public.ring(uuid) is 'Rings a contact (who must have the caller as a contact too) into the caller''s open call (refreshing its heartbeat, so a call stale for under 10 minutes comes back); otherwise it ends any open call and starts a new one. Ringing again restarts the 60 s; someone admitted or knocking is left as they are. Raises not_invited on an invite-only server for someone not on the invite list, even into a call they already host.';

create function public.ask_to_host()
returns void
language plpgsql
security definer
set search_path = ''
as $$
declare
  uid uuid := private.me();
begin
  insert into public.host_requests (user_id) values (uid) on conflict (user_id) do nothing;
end;
$$;
comment on function public.ask_to_host() is 'Tells the owner the caller would like to start calls (it shows in waiting_to_host). Asking again changes nothing, not even the time.';

create or replace function public.inbox(hosting uuid default null)
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
  -- went stale (a missed poll or two) if it was seen in the last 10 minutes. One silent for longer
  -- is over: it is ended, so nothing can bring it back. A wrong or foreign id is ignored.
  if inbox.hosting is not null then
    update public.calls k
      set seen_at = case when private.can_revive(k.ended_at, k.seen_at) then now() else k.seen_at end,
          ended_at = case when private.can_revive(k.ended_at, k.seen_at) then null else now() end
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
        where m.call_id = hosted.id
          and (m.state <> 'knocking' or m.seen_at > now() - interval '45 seconds')), '[]'::jsonb));
  end if;

  return jsonb_build_object(
    'me', private.profile_json(uid) || jsonb_build_object(
      'can_host', private.can_host(uid),
      'asked_to_host', exists (select 1 from public.host_requests r where r.user_id = uid)),
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
comment on function public.inbox(uuid) is 'Everything the app polls for, as one JSON object (see supabase/README.md); me carries can_host and asked_to_host. With hosting => the id of the call the app is sharing, also the host''s heartbeat for that call: it revives the call if it was seen in the last 10 minutes and ends it otherwise. Call it with POST.';

revoke all on function
  private.can_host(uuid),
  private.require_host(uuid),
  private.normalize_invited(),
  private.check_server_settings(),
  private.keep_invite_list()
from public, anon, authenticated;

revoke execute on function public.ask_to_host() from public, anon;
grant execute on function public.ask_to_host() to authenticated;
