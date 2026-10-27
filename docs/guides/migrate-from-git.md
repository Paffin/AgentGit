# Move from GitHub or GitLab

Start with a full copy and keep the source available until your team has verified the result. AgentGit never deletes or changes the source repository.

## In the browser

1. Open **Repositories → Import from GitHub / GitLab** as an administrator.
2. Paste the repository's HTTPS URL to get the two local commands below. Your existing Git credential manager authenticates private repositories; do not paste tokens into the page.
3. Run the commands in an empty directory, then upload `repository.bundle` and choose a new `owner/name`.
4. Review the branch/tag mapping and any content limitations. Save the JSON report.
5. Select **Publish repository**, then open it. No repository is visible to other users before publication.

```sh
git clone --mirror -- https://github.com/your-team/your-project.git agentgit-source.git
git -C agentgit-source.git bundle create ../repository.bundle --all
```

The same commands work with GitLab and self-hosted Git servers; replace the URL. For SSH, use the SSH URL with your local Git tooling and upload the resulting bundle. A full mirror includes branches and tags that an ordinary clone may keep only as remote-tracking refs. Shallow and incremental bundles are not complete migration inputs.

Browser imports currently support one server with SQLite and filesystem objects. Bundles are limited to 128 MiB; histories to 100,000 commits. One import is processed at a time and at most eight unpublished imports are retained. Unpublished staging is eligible for cleanup after 24 hours. Retry with the same browser action after a network interruption; publication is idempotent.

## From the terminal

For larger bundles or an offline migration:

```sh
ag migrate repository.bundle ./your-project --namespace your-team/your-project
cd your-project
ag status
ag verify
```

`ag migrate` also accepts a full local Git mirror or clone. Prefer a mirror to include every remote branch. The destination must not already exist. The command checks history, imports every branch and commit-based tag, verifies the result and prepares the default working tree. If content would be omitted, it refuses publication. Read the reported limitations before retrying with `--allow-lossy`.

## What moves, and what needs separate work

| Source data | Result |
|---|---|
| File history, authors, commit messages and merge parents | AgentGit changesets; shared history is converted once |
| Branches | Streams; names outside AgentGit's rules receive deterministic names listed in the report |
| Commit-based tags | `git-tag/<tag>` pins |
| Original commit IDs, signed tags and other refs | Original Git bundle retained in `.ag/import/source.bundle` |
| Mapping and content limitations | `.ag/import/report.json`; browser also downloads the complete receipt |
| Git LFS pointers | Pointer files only; transfer and retain external LFS storage separately |
| Submodules or unsupported file paths | Explicitly reported omissions requiring acknowledgement |
| Issues, pull/merge requests, comments, releases and permissions | Keep/export from the original host; not imported by this beta |
| GitHub Actions / GitLab CI | YAML files move as source; configure AgentGit validators and trusted signing keys separately |

The original bundle is included by `ag backup`. Preserve it: AgentGit IDs differ from Git IDs, and recreating Git history is not a substitute for preserving original signatures. Hosting account permissions are not mapped automatically; assign and check AgentGit roles before inviting the team.

## Verify before switching the team

Check the report's branch and tag counts, inspect important files, run your tests and `ag verify`, then make and restore a backup. Set policy and CI validation for the default stream. Try one small reviewed change. Keep GitHub/GitLab available for the metadata that has not moved and for rollback until your team accepts the migration.

To export a stream back to Git:

```sh
ag git export main ../git-export --branch main
```

Export recreates file history; it does not recreate hosting metadata, original Git IDs or signatures. Report a migration problem using the [bug template](https://github.com/Paffin/AgentGit/issues/new?template=bug_report.yml), with the release version and redacted report. Never attach private source or credentials.
