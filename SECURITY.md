# Security policy

## Supported versions

Ludion is pre-1.0. Only the latest `main` is supported.

## Threat model

- The editor is a trusted local tool. Its native commands, including the integrated terminal and
  file access, are not sandboxed. Open only projects you trust.
- The engine's wire server is unauthenticated. It binds `127.0.0.1:9411` by default; binding another
  address with `--addr` exposes it to that network.

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting on the repository's Security tab. Do not open a
public issue for a vulnerability.
