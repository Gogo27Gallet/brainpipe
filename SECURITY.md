# Security

BrainPipe is alpha software. Run document parsing in a restricted worker process
when handling untrusted inputs. Native PDF, image and Office parsers share the
worker's permissions. The HTTP API has no built-in authentication: bind it to
localhost or put an authenticated gateway in front of it. Enforce request-body,
concurrency and execution-time limits at that gateway.

The API copies uploads in blocks and rejects documents larger than 32 MiB;
this limit applies after multipart parsing and does not replace a gateway body
limit. It uses server-generated paths and disables persistent extraction caching
by default. If explicitly enabled, the cache contains document text on disk.
The library and CLI default to caching; use `use_cache=False` or `--no-cache`
for sensitive inputs. PII detection is heuristic and cannot guarantee removal
of every sensitive value.

Opt-in ZIP expansion allows at most 1,000 entries, 128 MiB per extracted member,
and 512 MiB total extracted data. Nested ZIPs are not expanded. These limits do
not apply to the internal ZIP parsers used by Office/EPUB formats.

For a suspected vulnerability, use GitHub's private vulnerability reporting on
this repository if available. Otherwise contact the maintainer privately before
posting exploit details. Do not attach real credentials or confidential files.
