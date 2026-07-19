# Contributing

Thank you for contributing to this project.

## Development

Install the supported runtime and development tools described in the
[README](README.md), then install dependencies:

```sh
just install
```

Use `just --list` to see the available development commands.

Before submitting a pull request, run:

```sh
just qa
git diff --check
```

Run `just mutation` when changing production logic or its focused tests.

Add or update tests for observable behavior, boundaries, failure recovery, and
security-sensitive output. Update applicable README content, examples, and API
documentation when public behavior changes.

Add meaningful user-visible changes to `CHANGELOG.md` under `[Unreleased]`.
Documentation and maintenance changes without user-visible impact do not need a
changelog entry.

## Pull requests

Keep each pull request focused.

Use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/)
formatting for commit messages and pull request titles:

```text
type[optional scope]: description
```

For example, `fix: preserve request ID`.

Complete the pull request template, including why the change is needed, what
changed, exact validation results, compatibility and security impact, and any
remaining risk.

Report security vulnerabilities using [SECURITY.md](SECURITY.md), not a public
issue.
