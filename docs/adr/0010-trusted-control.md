# ADR 0010: Trusted control for one call

Status: accepted (2026-11-26)

## Context
Until now the sharer answered every request for control: the guest asks, a prompt appears, the
sharer clicks Allow. That is right for a stranger, and tiresome in a pairing session where the
two people hand control back and forth every few minutes. Tuple and similar tools let the host
decide once that the person they are pairing with may take control.

Hard rule 4 still has to hold: the sharer explicitly grants control, sees who is controlling, and
can revoke it instantly with a global hotkey.

## Decision
- **The sharer can trust a guest with control for one call.** The app sends
  `trust_control {identity}` ("Let in with control" on admitting someone, "Give control" during
  the call). From then on that guest's requests are granted at once, with no prompt. If the
  guest is asking right now, they get control now, and that prompt's request number is closed.
- **It is still the guest who takes control.** Trust grants nothing by itself. Nobody is put in
  control without sending a request of their own, and a trusted guest who asks while someone
  else has control is turned down, as anyone is: one controller at a time.
- **Only the sharer's app can give it.** `trust_control` comes only from the engine's stdin, so
  only the app that started the engine can send it. There is no protocol message for it: nothing
  a guest sends, and nothing from the server, can create or restore trust. It is refused (with an
  `error` event) without `--allow-control`, for anyone who isn't a guest in the call (the sharer
  itself included), and while someone else has control.
- **Every revoke takes it all back.** The revoke hotkey (⌃⌥⌘R), `revoke_control` and "Take back"
  clear every trust, in every state: with someone in control, someone asking, or nobody, so a
  revoke pressed after "Let in with control" but before the guest used it still counts. A revoke
  that happens after the app sent `trust_control`, even one the control thread carries out
  before it reads the trust, wins: that trust is dropped. After any revoke the next request is
  a prompt again.
- **A promise for someone who hasn't arrived yet lives in the engine too.** "Let in with
  control" usually comes before the guest is in the media room. The app sends
  `trust_on_arrival {identity}`: for someone already in the call it is `trust_control`;
  otherwise the engine trusts that guest if they join within a minute, and drops the promise
  silently after that. Every revoke cancels it, in every state, also before the guest arrives:
  the revoke hotkey never passes through the app, so the promise can't be kept anywhere the
  hotkey doesn't reach. The guest leaving cancels it too, and nothing they send (a hello, a
  reconnect) extends it. It is refused as `trust_control` is, and when the guest joins while
  someone else has control, they are not trusted and the promise is gone.
- **It ends with the call.** Giving control back keeps the trust, so the guest can take it again.
  Leaving the call ends it: a guest who comes back, even with the same identity (a call pass
  gives one per account), is asked again. Trust lives in the engine's memory only; it is never
  written to disk, the environment or the server, and a new call starts with nobody trusted.
- **The app is told who is trusted.** The sharer's `control` event carries `trusted`, the guests
  trusted right now, and is sent again whenever that changes.
- **Names come from the token.** The prompt, the `control` event, `trusted` and the on-screen
  label name the guest as their token does (with a call pass, their account's name), not as
  their own hello says. Trusting someone by name only makes sense if the name is not the guest's
  own claim.
- The rules live in `crates/control`'s `Controller`, next to the rest of consent, and are covered
  by its trace tests; `bench/engine-e2e.sh` runs them over a real call.

## Consequences
- How each part of hard rule 4 still holds:
  - *The sharer explicitly grants control.* Trust is an explicit grant, given by the sharer, for
    one named guest, for one call. It replaces the prompt for that guest's later requests; it is
    not a default and nothing turns it on for anyone else.
  - *The sharer sees who is controlling.* A trusted grant is the same grant as any other: the
    same "… is controlling this computer" label for as long as it lasts, the same `control` event,
    the same input checks, deadman and lateness rules. The app also shows who is trusted.
  - *The sharer can revoke it instantly with a global hotkey.* Unchanged, and it now ends the trust
    too. Revoking depends on nothing but the hotkey and the revoke switch.
- What trusted control deliberately does not do: it never lasts beyond the call, is never
  remembered for a person or an account, is never set by the server, an invitation or a guest,
  and never starts control by itself.
- A trusted guest can take control while the sharer is looking away. That is what the sharer
  chose; the label still shows it, and anyone at the Mac can take it back.
- In key-and-secret mode the token's name is whatever the guest typed, so a name is still not an
  identity there; call passes make it the account's name.
