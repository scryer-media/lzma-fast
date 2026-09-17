# Security policy

## Supported versions

Only the latest published release of `lzma-turbo` receives fixes.

## Reporting a vulnerability

Please do not open a public issue for security problems. Use GitHub's private
vulnerability reporting on this repository (Security tab, "Report a
vulnerability"). You will get an acknowledgement within a few days.

A decoder's attack surface is its input. Reports of panics, out-of-bounds
reads, unbounded allocation or hangs on crafted LZMA/LZMA2 streams are
security reports and are handled as such. Include the input if you can; a
fuzz artifact is ideal.
