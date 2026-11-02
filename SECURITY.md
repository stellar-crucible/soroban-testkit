# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.4.x   | :white_check_mark: |
| < 0.4   | :x:                |

Only the latest published minor is supported. This crate is pre-1.0, so a minor bump may change public API; security fixes land in a new minor rather than being back-ported to an older one.

## Reporting a Vulnerability

If you discover a security vulnerability in Soroban Testkit, please report it responsibly.

**Do not open a public GitHub issue for security vulnerabilities.**

Instead, please email the maintainers directly or use GitHub's private vulnerability reporting feature on this repository.

We aim to acknowledge reports within 48 hours and provide an initial assessment within one week.

## Scope

This security policy covers:
- The testkit crates and their dependencies
- CI/CD workflows
- Documentation site

Out of scope:
- Vulnerabilities in the Soroban SDK itself (report to Stellar)
- Issues in downstream projects using testkit
