# Security policy

## Supported versions

Only the latest published release of `lzma-turbo` receives fixes.

## Reporting a vulnerability

Please do not open a public issue for security problems. Report privately
through GitHub at <https://github.com/scryer-media/lzma-turbo/security/advisories/new>
(the Security tab, "Report a vulnerability"); private reporting is enabled on
this repository. You will get an acknowledgement within a few days, and a
fix ships as a new release of the crate, credited to you unless you ask
otherwise.

## The threat model, and what bounds it

[`docs/security.md`](docs/security.md) lists every limit this crate places on
an input, with its default, the attack it closes, and where it is enforced.
Read it before reporting, and before pointing this crate at untrusted input.

A decoder's attack surface is its input. Reports of panics, out-of-bounds
reads, unbounded allocation or hangs on crafted LZMA/LZMA2 streams are
security reports and are handled as such. Include the input if you can; a
fuzz artifact is ideal.
