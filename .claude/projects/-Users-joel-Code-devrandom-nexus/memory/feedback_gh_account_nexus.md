---
name: Use joeldsouzax gh account for nexus
description: Use joeldsouzax (not trivejoel) as the active gh CLI account when working in the nexus repo
type: feedback
---

Use `joeldsouzax` as the active `gh` CLI account for the nexus repo (`devrandom-labs/nexus`). `trivejoel` is for other repos.

**Why:** User has two GitHub accounts and wants `joeldsouzax` associated with this project's GitHub operations (PRs, visibility, issues, etc.).

**How to apply:** Before any `gh` API call in this repo, verify `gh auth status` shows `joeldsouzax` as active. If not, run `gh auth switch --user joeldsouzax` first.
