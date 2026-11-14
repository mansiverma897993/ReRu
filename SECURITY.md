# Security policy

## Supported versions

ReRu is an experimental preview. Security fixes land on `main` and, when a
release exists, in the newest published pre-release.

| Version | Supported |
| --- | --- |
| `main` (unreleased) | Yes |
| `0.1.0-alpha.1` | Best effort |

## Reporting a vulnerability

**Do not open a public issue for a security problem.**

Report privately using GitHub's
[private vulnerability reporting](https://github.com/mansiverma897993/ReRu/security/advisories/new)
or by contacting an owner at
[@mansiverma897993](https://github.com/mansiverma897993) or
[@MohitGoyal09](https://github.com/MohitGoyal09). A report about one owner goes
to the other.

Please include:

- ReRu version and commit (`reru --version`, `git rev-parse HEAD`)
- Operating system and Rust version
- A minimal reproduction: startup flags, commands, and observed output
- The impact you believe it has
- Whether you want public credit

You can expect an acknowledgement within 7 days and a status update within 30
days. We will coordinate a disclosure date with you before publishing details.

## Scope

ReRu listens on `127.0.0.1` only and is designed for local development. Issues
that matter most:

- Authentication bypass or credential leakage with `--requirepass`
- Remote crash from a crafted RESP message
- Path traversal through `--snapshot`, `--appendonly`, `--dir`, or `--dbfilename`
- Data exposure between clients, for example a replica leaking its primary
  password

## Known limitations (not vulnerabilities)

These are documented gaps, not security bugs:

- No TLS. Traffic, including `AUTH` passwords, is plain text on the network.
- No multi-user ACLs; `AUTH` covers the default user only.
- No sandboxing of persistence paths. Run ReRu with the filesystem permissions
  you intend.
- Memory limits use a logical dataset estimate, not process RSS, so a process
  can exceed `--maxmemory`.
- ReRu binds to loopback by default. `--bind 0.0.0.0` is intended for trusted
  container networks only, not for public exposure.

## Hardening guidance

If you expose ReRu beyond your machine, place it behind a trusted network
boundary, set a password with `--requirepass-file`, and keep the loopback bind.
