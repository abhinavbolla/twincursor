# Security policy

TwinCursor shares screens and lets another person control your computer, so we take security
reports seriously.

## Reporting a vulnerability

**Please don't open a public issue.** Report it privately through GitHub:
[Report a vulnerability](https://github.com/abhinavbolla/twincursor/security/advisories/new)
(Security tab → "Report a vulnerability").

Please include:
- what an attacker could do, and under which conditions;
- steps to reproduce, or a proof of concept;
- the version or commit you tested.

We'll acknowledge your report within 7 days. We'll keep you updated while we fix it, and credit
you in the advisory unless you'd rather stay anonymous.

## Scope

Especially relevant:
- **Remote control without consent:** input injected without the sharer's grant, or control
  that can't be revoked.
- Screen content or audio reaching anyone not in the call.
- Problems with the server's authentication, rooms or LiveKit tokens.

TwinCursor is pre-release. There are no supported versions yet; fixes land on `main`.
