---
description: Update CHANGELOG.md with a new entry for the current change
user_invocable: true
---

# Update Changelog

Add a new entry to `CHANGELOG.md` for the change being made in the current branch.

## Instructions

1. Read `CHANGELOG.md` to understand the current format.
2. Determine the appropriate category for the change:
   - **Added** — new user-facing feature or capability
   - **Changed** — modification to existing user-facing behavior
   - **Removed** — removal of user-facing feature or option
   - **Fixed** — bug fix affecting users
   - **Perf** — performance improvement noticeable to users
3. Write a concise, user-facing description. Focus on what changed from the user's perspective, not implementation details.
4. Include a PR link in the format `([#NNN](https://github.com/voidzero-dev/vite-task/pull/NNN))`. If the PR number is not yet known, leave a `([#???](https://github.com/voidzero-dev/vite-task/pull/???))` placeholder.
5. Append the new entry at the end of the existing list in `CHANGELOG.md`.
6. If the current change is closely related to an existing entry (e.g., multiple PRs contributing to the same feature or fix), group them into a single item with multiple PR links rather than adding a separate entry.

## What NOT to include

Do not add entries for:

- Internal refactors with no user-facing effect
- CI/CD changes
- Dependency bumps
- Test-only fixes (flaky tests, test infrastructure)
- Documentation changes (CLAUDE.md, README, etc.)
- Chore/tooling changes

The changelog is for **end-users only**.

## Entry format

```
- **Category** description ([#NNN](https://github.com/voidzero-dev/vite-task/pull/NNN))
```
