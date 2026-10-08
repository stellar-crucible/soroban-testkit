# Soroban Testkit Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `testkit-core`: BudgetSnapshot for CPU/memory tracking
- `testkit-core`: DecodedError for human-readable error messages
- `testkit-core`: Storage inspection utilities
- `testkit-assert`: EventMatcher for ergonomic event assertions
- `testkit-assert`: AuthMatcher for authorization verification
- `testkit-fixtures`: TestContext and TestContextBuilder
- `testkit-generators`: proptest strategies for token amounts, ledger sequences, timestamps
- mdBook documentation site with guides and API references
- CI workflow (fmt, clippy, test)
- Docs deployment workflow via GitHub Pages
- Issue templates for bug reports and feature requests
- Drips Wave complexity labels (trivial/medium/high)
